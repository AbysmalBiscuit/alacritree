# Taskwarrior task tracking implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Agents and humans share hierarchical task lists stored in taskwarrior, edited from a Ctrl+~ tab in alacritree and fed to Claude Code and codex through `alacritree hook <event>`.

**Architecture:** A new `tasks/` module holds `scope` (pure naming), `facts` (git facts for a cwd, read on the cwd's side), `taskwarrior` (the only code that runs `task`), `tree` (the tab's pure model), `hook` (what `alacritree hook` prints) and `view` (the egui tab). The CLI adds `task scope`, `task setup` and `hook`, all local commands. The tab is a PTY-less `SessionKind::Tasks`, drawn by `tasks/view.rs` the way the scratchpad is drawn by `scratchpad.rs`.

**Tech Stack:** Rust 2024, egui, serde/serde_json, clap, the crate's `jobs` pool and `command_ext::hidden`, taskwarrior 3 (`task`), the git CLI.

**Spec:** `docs/superpowers/specs/2026-09-21-taskwarrior-tasks-design.md`

## Global Constraints

- Off by default: `[integrations.taskwarrior] enabled = false`. With it off, `OpenTasks` does nothing and the palette omits it.
- Every child process is built with `command_ext::hidden` and waited on from a `jobs` pool worker (`jobs::pool().spawn`) or, in the CLI, inside `jobs::on_this_thread`. clippy disallows `Command::new`, `spawn`, `output` and `status` elsewhere.
- Every `task` invocation passes `rc.uda.subof.type=uuid`, `rc.uda.subof.label=Sub of`, `rc.uda.order.type=numeric`, `rc.uda.order.label=Order` and `rc.confirmation=off`. `add` also passes `rc.verbose=new-uuid`.
- The UDA is `subof`, never `parent`, which is a core taskwarrior attribute.
- `order` is an integer with a stride of 1024 between siblings.
- Project nodes: `global`, `<repo>`, `<repo>.<branch>`, `<repo>.<branch>.<claude|codex>-<id>`. `.`, `/` and `\` in `<repo>`, `<branch>` and `<id>` become `-`.
- Codex's shell-side id is `CODEX_SESSION_ID`, Claude's is `CLAUDE_CODE_SESSION_ID`. The hook uses the stdin payload's `session_id` and `--harness`.
- The tab's export filter is `(project.is:<repo> or project:<repo>. or project.is:global) (status:pending or status:completed)`.
- Hook stdout is exactly one JSON object `{"hookSpecificOutput":{"hookEventName":"<SessionStart|UserPromptSubmit>","additionalContext":"..."}}` or nothing. The hook always exits 0.
- New config keys carry defaults in the `Raw*` `Default` impl under `#[serde(default)]`, with doc comments. Regenerate the schema with `ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema`.
- Build, test and format through devkit: `devkit run task fmt`, `devkit run task check`, `devkit run task test`, `devkit run task clippy`, each with `--dir <worktree>` when run from the main checkout. Never disable the rustc cache through an env var.
- Comments explain why, never what. No em dashes, interpuncts or ellipsis characters in new comments or docs.
- Commits are Conventional Commits ending with `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.

## Review Focus

1. A task whose `subof` names a uuid missing from the export (deleted, or in another scope) must render as a root row, never vanish. Pinned by `orphan_subof_renders_as_root` in Task 5.
2. A `subof` cycle written by an agent (A under B, B under A) must terminate and show both rows. Pinned by `subof_cycle_terminates` in Task 5.
3. A description containing taskwarrior syntax such as `project:x`, `+tag` or `due:tomorrow` must be stored verbatim, on add and on edit. Pinned by `description_with_attribute_syntax_is_verbatim` and `describe_replaces_the_text_verbatim` in Task 3.
4. A branch name with `/` and `.` (`feat/v1.2`) and a session id with `.` must keep the node four levels deep. Pinned by `dots_and_slashes_never_add_levels` in Task 1.
5. The hook run with empty or non-JSON stdin must exit 0 and print nothing or one valid JSON object. Pinned by `garbage_stdin_exits_zero` in Task 6.

## Setup

- [ ] Find the stack base and cut the worktree:

```sh
gh pr list --repo mathix420/alacritree --state open --json number,title,headRefName
devkit issue setup 86 --summary --slug feat/86-taskwarrior-tasks
```

The open PR with the highest `[n]` marker is the base, and this branch's marker is `n + 1`. With no PR open, the base is `master`. Re-point the branch before it has commits:

```sh
git -C C:/Users/Lev/Git/github/alacritree-worktrees/feat/86-taskwarrior-tasks reset --hard origin/<base>
```

`<worktree>` below means that path.

- [ ] Confirm `task --version` prints 3.x. The adapter and hook tests return early without it, so install it to see them run.

---

### Task 1: Scope naming

**Files:**
- Create: `alacritree/src/tasks/mod.rs`
- Create: `alacritree/src/tasks/scope.rs`
- Modify: `alacritree/src/lib.rs` (`pub(crate) mod tasks;` next to `pub(crate) mod scratchpad;`)

**Interfaces:**
- Produces:
  - `scope::Harness { Claude, Codex }`, `Harness::parse(&str) -> Option<Harness>`, `Harness::prefix(self) -> &'static str`
  - `scope::SessionRef { harness: Harness, id: String }`
  - `scope::Place { Global, Project { repo: String }, Workspace { repo: String, branch: String } }`
  - `scope::GLOBAL: &str = "global"`
  - `scope::sanitize(&str) -> String`
  - `scope::node(&Place, Option<&SessionRef>) -> String`
  - `scope::session_from_env(impl Fn(&str) -> Option<String>) -> Option<SessionRef>`

- [ ] **Step 1: Write the failing tests** at the bottom of `alacritree/src/tasks/scope.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn ws(repo: &str, branch: &str) -> Place {
        Place::Workspace { repo: repo.into(), branch: branch.into() }
    }

    fn codex(id: &str) -> SessionRef {
        SessionRef { harness: Harness::Codex, id: id.into() }
    }

    #[test]
    fn each_place_names_its_node() {
        assert_eq!(node(&Place::Global, None), "global");
        assert_eq!(node(&Place::Project { repo: "alacritree".into() }, None), "alacritree");
        assert_eq!(node(&ws("alacritree", "master"), None), "alacritree.master");
        assert_eq!(
            node(&ws("alacritree", "master"), Some(&codex("0199a"))),
            "alacritree.master.codex-0199a"
        );
    }

    #[test]
    fn a_session_below_anything_but_a_workspace_is_dropped() {
        assert_eq!(node(&Place::Global, Some(&codex("x"))), "global");
        assert_eq!(node(&Place::Project { repo: "r".into() }, Some(&codex("x"))), "r");
    }

    #[test]
    fn dots_and_slashes_never_add_levels() {
        let session = SessionRef { harness: Harness::Claude, id: "a.b/c".into() };
        let name = node(&ws("my.repo", "feat/v1.2"), Some(&session));
        assert_eq!(name, "my-repo.feat-v1-2.claude-a-b-c");
        assert_eq!(name.matches('.').count(), 2);
    }

    #[test]
    fn codex_session_id_wins_over_claude() {
        let env = |key: &str| match key {
            "CODEX_SESSION_ID" => Some("c1".to_string()),
            "CLAUDE_CODE_SESSION_ID" => Some("k1".to_string()),
            _ => None,
        };
        assert_eq!(session_from_env(env), Some(codex("c1")));
    }

    #[test]
    fn claude_session_id_is_read_when_codex_is_absent() {
        let env = |key: &str| (key == "CLAUDE_CODE_SESSION_ID").then(|| "k1".to_string());
        assert_eq!(
            session_from_env(env),
            Some(SessionRef { harness: Harness::Claude, id: "k1".into() })
        );
    }

    #[test]
    fn blank_ids_are_no_session() {
        let env = |key: &str| (key == "CODEX_SESSION_ID").then(|| "  ".to_string());
        assert_eq!(session_from_env(env), None);
    }

    #[test]
    fn harness_names_round_trip() {
        assert_eq!(Harness::parse("claude"), Some(Harness::Claude));
        assert_eq!(Harness::parse("codex"), Some(Harness::Codex));
        assert_eq!(Harness::parse("gemini"), None);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `devkit run task test --dir <worktree> -- tasks::scope`
Expected: FAIL to compile, `node`, `Place` and `Harness` not found.

- [ ] **Step 3: Implement.** `alacritree/src/tasks/mod.rs`:

```rust
//! Task lists kept in taskwarrior and shared between agents and humans.
//! Taskwarrior owns the tasks; this module names where one belongs, runs
//! `task`, and shapes the result for the tab and the agent hooks.

pub(crate) mod scope;
```

`alacritree/src/tasks/scope.rs`, above the tests:

```rust
//! Where a task lives, as a taskwarrior project node.  The CLI, the hook and
//! the tab each gather facts their own way and meet here, so all three name
//! a place the same way.

pub(crate) const GLOBAL: &str = "global";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Harness {
    Claude,
    Codex,
}

impl Harness {
    pub(crate) fn parse(name: &str) -> Option<Self> {
        match name {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }

    pub(crate) fn prefix(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

/// An agent conversation, keyed by the harness's own id so a resumed
/// conversation finds its tasks again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionRef {
    pub harness: Harness,
    pub id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Place {
    Global,
    Project { repo: String },
    Workspace { repo: String, branch: String },
}

/// Taskwarrior splits project names on `.`, so a dot inside one segment would
/// invent a level.
pub(crate) fn sanitize(segment: &str) -> String {
    segment.chars().map(|c| if matches!(c, '.' | '/' | '\\') { '-' } else { c }).collect()
}

/// A session only means something inside a workspace; above one there is no
/// conversation to key.
pub(crate) fn node(place: &Place, session: Option<&SessionRef>) -> String {
    match place {
        Place::Global => GLOBAL.to_string(),
        Place::Project { repo } => sanitize(repo),
        Place::Workspace { repo, branch } => {
            let workspace = format!("{}.{}", sanitize(repo), sanitize(branch));
            match session {
                Some(s) => format!("{workspace}.{}-{}", s.harness.prefix(), sanitize(&s.id)),
                None => workspace,
            }
        },
    }
}

/// Codex's hook payload carries the root session id, which is also what it
/// exports as `CODEX_SESSION_ID`, so keying on it keeps a subagent's shell
/// and the hook on one node.
pub(crate) fn session_from_env(get: impl Fn(&str) -> Option<String>) -> Option<SessionRef> {
    [("CODEX_SESSION_ID", Harness::Codex), ("CLAUDE_CODE_SESSION_ID", Harness::Claude)]
        .into_iter()
        .find_map(|(key, harness)| {
            let id = get(key)?.trim().to_string();
            (!id.is_empty()).then_some(SessionRef { harness, id })
        })
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `devkit run task test --dir <worktree> -- tasks::scope`
Expected: 7 passed.

- [ ] **Step 5: Commit**

```sh
git -C <worktree> add alacritree/src/tasks alacritree/src/lib.rs
git -C <worktree> commit -m "feat(tasks): name taskwarrior scopes" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 2: Config and the `task` tool

**Files:**
- Modify: `alacritree/src/tools.rs` (`Tool::Task`, every `6` array length to `7`)
- Modify: `alacritree/src/wsl_helper.rs:17` (`HELLO_TOOLS` gains `"task"`)
- Modify: `alacritree/src/config.rs` (`RawTaskwarrior`, `TaskwarriorConfig`, `RawIntegrations`, `IntegrationsConfig`, `paths`, `tool_paths`)
- Modify: `alacritree/src/cli/doctor.rs:863,885,895` and `alacritree/src/app/git_panel.rs:1444` (`6` to `7`)
- Regenerate: `schema/alacritree-config.json`, plus `alacritree/tests/stock-config.json` if the schema test asks for it

**Interfaces:**
- Produces:
  - `tools::Tool::Task`, `Tool::Task.name() == "task"`, `Tool::ALL: [Tool; 7]`
  - `config::TaskwarriorConfig { pub path: String, pub wsl_path: Option<String>, pub enabled: bool }`
  - `Config.integrations.taskwarrior: TaskwarriorConfig`

- [ ] **Step 1: Write the failing tests** in `config.rs`'s test module, beside the `paths` tests near line 4698. Parse TOML with the same helper the neighbouring `[integrations.tuicr]` test near line 4726 uses; `parse` below stands for it:

```rust
#[test]
fn taskwarrior_is_off_and_named_by_default() {
    let config = Config::default();
    assert!(!config.integrations.taskwarrior.enabled);
    assert_eq!(config.integrations.paths(Tool::Task), ToolPaths::named(Tool::Task));
}

#[test]
fn taskwarrior_table_sets_both_sides() {
    let config = parse(
        "[integrations.taskwarrior]\nenabled = true\npath = 'C:/bin/task.exe'\nwsl_path = '/usr/bin/task'\n",
    );
    assert!(config.integrations.taskwarrior.enabled);
    assert_eq!(
        config.integrations.paths(Tool::Task),
        ToolPaths { native: "C:/bin/task.exe".into(), wsl: Some("/usr/bin/task".into()) }
    );
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `devkit run task test --dir <worktree> -- taskwarrior_`
Expected: FAIL to compile, `Tool::Task` and the `taskwarrior` field are missing.

- [ ] **Step 3: Implement.** `tools.rs`: add `Task` as the last variant, `Tool::Task => "task"` in `name`, and

```rust
pub const ALL: [Tool; 7] =
    [Tool::Git, Tool::Gh, Tool::Delta, Tool::Doppler, Tool::Herdr, Tool::Tuicr, Tool::Task];
```

with `[ToolPaths; 7]` in `configured`, `configure` and `test_configuration`. `wsl_helper.rs`:

```rust
pub const HELLO_TOOLS: [&str; 7] = ["git", "gh", "delta", "doppler", "herdr", "tuicr", "task"];
```

`config.rs`, beside `RawHerdr`:

```rust
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(default)]
struct RawTaskwarrior {
    /// The program to run on Windows or natively. Its own name is looked up
    /// on PATH; any other value runs as written.
    path: String,
    /// The program to run inside every WSL distro, as written. Empty finds it
    /// by name through the distro's login shell.
    wsl_path: String,
    /// Show task lists kept in taskwarrior in a tab (`OpenTasks`, Ctrl+~).
    /// Agents write the same lists with `task` and read them through
    /// `alacritree hook`. Off leaves the binding inert and the palette entry
    /// out.
    enabled: bool,
}

impl Default for RawTaskwarrior {
    fn default() -> Self {
        Self { path: "task".to_string(), wsl_path: String::new(), enabled: false }
    }
}

impl RawTaskwarrior {
    fn resolve(self) -> TaskwarriorConfig {
        let tool = tool_config(self.path, self.wsl_path, Tool::Task);
        TaskwarriorConfig { path: tool.path, wsl_path: tool.wsl_path, enabled: self.enabled }
    }
}
```

and beside `HerdrConfig`:

```rust
/// `[integrations.taskwarrior]`: where `task` lives on each side, and
/// whether the tasks tab is on.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct TaskwarriorConfig {
    pub path: String,
    pub wsl_path: Option<String>,
    pub enabled: bool,
}
```

Then add `/// Task lists kept in taskwarrior.` plus `taskwarrior: RawTaskwarrior,` to `RawIntegrations`, `pub taskwarrior: TaskwarriorConfig,` to `IntegrationsConfig`, `taskwarrior: self.taskwarrior.resolve(),` to `RawIntegrations::resolve`, `Tool::Task => (&self.taskwarrior.path, &self.taskwarrior.wsl_path),` to `IntegrationsConfig::paths`, and `[ToolPaths; 7]` to `tool_paths`. Change the `[None; 6]` in the three `doctor.rs` tests and `[crate::tools::ToolPaths; 6]` at `git_panel.rs:1444` to `7`.

- [ ] **Step 4: Regenerate the schema and run**

Run: `ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema`, then `devkit run task test --dir <worktree>`
Expected: the two new tests, `config_schema` and `schema_defaults` pass, and nothing else changes status. If a doctor test prints the tool list, add `task` to its expected output.

- [ ] **Step 5: Commit**

```sh
git -C <worktree> add -A alacritree schema
git -C <worktree> commit -m "feat(config): add the taskwarrior integration table" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 3: The taskwarrior adapter

**Files:**
- Create: `alacritree/src/tasks/taskwarrior.rs`
- Modify: `alacritree/src/tasks/mod.rs` (`pub(crate) mod taskwarrior;`)

**Interfaces:**
- Consumes: `tools::{Tool::Task, program, wsl_in_job}`, `multiplexer::Side`, `jobs::Blocking`, `command_ext::hidden`.
- Produces:
  - `Status { Pending, Completed, Other }`
  - `Task { uuid, description, status: Status, start: Option<String>, subof: Option<String>, order: Option<i64>, project: Option<String>, entry: Option<String>, modified: Option<String> }` (`Deserialize`, `Clone`, `PartialEq`, `Debug`)
  - `TaskError { Missing { program: String }, Failed { stderr: String }, Io(String) }` with `Display`
  - `UDA_DECLARATIONS: [(&str, &str); 4]`
  - `Taskwarrior::for_side(Side, &Blocking) -> Taskwarrior`, `with_env(self, &str, &str) -> Taskwarrior`
  - `export(&self, filter: &[String], &Blocking) -> Result<Vec<Task>, TaskError>`
  - `add(&self, project: &str, description: &str, subof: Option<&str>, order: i64, &Blocking) -> Result<String, TaskError>` (the new uuid)
  - `modify(&self, uuid: &str, mods: &[String], &Blocking) -> Result<(), TaskError>`
  - `describe(&self, uuid: &str, text: &str, &Blocking) -> Result<(), TaskError>`
  - `done`, `undone`, `start`, `stop`, `delete`: `(&self, uuid: &str, &Blocking) -> Result<(), TaskError>`
  - `rc_value(&self, key: &str, &Blocking) -> Result<Option<String>, TaskError>` (reads the taskrc, ignoring this adapter's overrides)
  - `set_config(&self, key: &str, value: &str, &Blocking) -> Result<(), TaskError>`

- [ ] **Step 1: Write the failing tests** at the bottom of `taskwarrior.rs`. The integration tests use a private `TASKRC` and `TASKDATA` and return early when `task` is not installed:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs;

    /// A private taskwarrior, so the user's rc, data and hooks never run.
    fn private() -> Option<(tempfile::TempDir, Taskwarrior)> {
        let dir = tempfile::tempdir().expect("a temp dir");
        let rc = dir.path().join("taskrc");
        std::fs::write(&rc, "").expect("an empty taskrc");
        let tw = jobs::on_this_thread(|b| Taskwarrior::for_side(Side::Native, b))
            .with_env("TASKRC", rc.to_str().unwrap())
            .with_env("TASKDATA", dir.path().join("data").to_str().unwrap());
        let installed = jobs::on_this_thread(|b| tw.export(&[], b)).is_ok();
        installed.then_some((dir, tw))
    }

    fn find(tw: &Taskwarrior, uuid: &str) -> Option<Task> {
        jobs::on_this_thread(|b| tw.export(&[], b)).unwrap().into_iter().find(|t| t.uuid == uuid)
    }

    #[test]
    fn add_returns_the_uuid_and_round_trips_subof_and_order() {
        let Some((_dir, tw)) = private() else { return };
        let parent = jobs::on_this_thread(|b| tw.add("r.main", "ship", None, 1024, b)).unwrap();
        let child =
            jobs::on_this_thread(|b| tw.add("r.main", "spec", Some(&parent), 2048, b)).unwrap();
        let task = find(&tw, &child).expect("child exported");
        assert_eq!(task.subof.as_deref(), Some(parent.as_str()));
        assert_eq!(task.order, Some(2048));
        assert_eq!(task.project.as_deref(), Some("r.main"));
        assert_eq!(task.status, Status::Pending);
    }

    #[test]
    fn description_with_attribute_syntax_is_verbatim() {
        let Some((_dir, tw)) = private() else { return };
        let text = "fix project:x +tag due:tomorrow";
        let uuid = jobs::on_this_thread(|b| tw.add("r", text, None, 1024, b)).unwrap();
        let task = find(&tw, &uuid).unwrap();
        assert_eq!(task.description, text);
        assert_eq!(task.project.as_deref(), Some("r"));
    }

    #[test]
    fn describe_replaces_the_text_verbatim() {
        let Some((_dir, tw)) = private() else { return };
        let uuid = jobs::on_this_thread(|b| tw.add("r", "old", None, 1024, b)).unwrap();
        jobs::on_this_thread(|b| tw.describe(&uuid, "new +not-a-tag", b)).unwrap();
        assert_eq!(find(&tw, &uuid).unwrap().description, "new +not-a-tag");
    }

    #[test]
    fn every_verb_works_by_uuid_without_prompting() {
        let Some((_dir, tw)) = private() else { return };
        let uuid = jobs::on_this_thread(|b| tw.add("r", "t", None, 1024, b)).unwrap();
        jobs::on_this_thread(|b| tw.start(&uuid, b)).unwrap();
        assert!(find(&tw, &uuid).unwrap().start.is_some());
        jobs::on_this_thread(|b| tw.stop(&uuid, b)).unwrap();
        assert!(find(&tw, &uuid).unwrap().start.is_none());
        jobs::on_this_thread(|b| tw.done(&uuid, b)).unwrap();
        assert_eq!(find(&tw, &uuid).unwrap().status, Status::Completed);
        jobs::on_this_thread(|b| tw.undone(&uuid, b)).unwrap();
        assert_eq!(find(&tw, &uuid).unwrap().status, Status::Pending);
        jobs::on_this_thread(|b| tw.modify(&uuid, &["order:7".into()], b)).unwrap();
        assert_eq!(find(&tw, &uuid).unwrap().order, Some(7));
        jobs::on_this_thread(|b| tw.delete(&uuid, b)).unwrap();
        assert_eq!(find(&tw, &uuid).unwrap().status, Status::Other);
    }

    #[test]
    fn set_config_is_visible_to_rc_value_and_overrides_are_not() {
        let Some((_dir, tw)) = private() else { return };
        let before = jobs::on_this_thread(|b| tw.rc_value("uda.subof.type", b)).unwrap();
        assert_eq!(before, None, "the adapter's own overrides must not count as declared");
        jobs::on_this_thread(|b| tw.set_config("uda.subof.type", "uuid", b)).unwrap();
        let after = jobs::on_this_thread(|b| tw.rc_value("uda.subof.type", b)).unwrap();
        assert_eq!(after.as_deref(), Some("uuid"));
    }

    #[test]
    fn a_missing_program_is_reported_as_missing() {
        let mut tw = jobs::on_this_thread(|b| Taskwarrior::for_side(Side::Native, b));
        tw.program = "alacritree-no-such-task-binary".into();
        let err = jobs::on_this_thread(|b| tw.export(&[], b)).unwrap_err();
        assert!(matches!(err, TaskError::Missing { .. }), "{err}");
    }

    #[test]
    fn busy_stderr_is_retryable_and_others_are_not() {
        assert!(is_busy("database is locked"));
        assert!(is_busy("Error: SQLITE_BUSY"));
        assert!(!is_busy("No matches."));
    }

    #[test]
    fn the_new_uuid_is_read_from_verbose_output() {
        let out = "Created task 8e4a5a4e-1f2b-4c3d-9e8f-0a1b2c3d4e5f.\n";
        assert_eq!(created_uuid(out).as_deref(), Some("8e4a5a4e-1f2b-4c3d-9e8f-0a1b2c3d4e5f"));
        assert_eq!(created_uuid("Created task 5."), None);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `devkit run task test --dir <worktree> -- tasks::taskwarrior`
Expected: FAIL to compile, `Taskwarrior` not found.

- [ ] **Step 3: Implement** the module above the tests:

```rust
//! The only code that runs `task`.  Every call is addressed by uuid, declares
//! the UDAs itself so a taskrc without them never folds `subof:` into a
//! description, and turns off prompts, since there is no terminal to answer
//! them.

use std::io;
use std::process::Output;
use std::time::Duration;

use serde::Deserialize;

use crate::command_ext::hidden;
use crate::jobs::Blocking;
use crate::multiplexer::Side;
use crate::tools::{self, Tool};

/// Also what `alacritree task setup` writes into each side's taskrc.
pub(crate) const UDA_DECLARATIONS: [(&str, &str); 4] = [
    ("uda.subof.type", "uuid"),
    ("uda.subof.label", "Sub of"),
    ("uda.order.type", "numeric"),
    ("uda.order.label", "Order"),
];

/// taskchampion takes an immediate write lock with no busy timeout, so a
/// second writer fails at once instead of waiting its turn.
const BUSY_RETRIES: [Duration; 3] =
    [Duration::from_millis(50), Duration::from_millis(150), Duration::from_millis(400)];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Status {
    Pending,
    Completed,
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub(crate) struct Task {
    pub uuid: String,
    pub description: String,
    pub status: Status,
    pub start: Option<String>,
    pub subof: Option<String>,
    #[serde(default, deserialize_with = "integer_order")]
    pub order: Option<i64>,
    pub project: Option<String>,
    pub entry: Option<String>,
    pub modified: Option<String>,
}

/// Numeric UDAs export as JSON numbers that may carry a fraction.
fn integer_order<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    Ok(Option::<f64>::deserialize(d)?.map(|n| n.round() as i64))
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TaskError {
    Missing { program: String },
    Failed { stderr: String },
    Io(String),
}

impl std::fmt::Display for TaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing { program } => write!(f, "taskwarrior not found: {program}"),
            Self::Failed { stderr } => write!(f, "task failed: {}", stderr.trim()),
            Self::Io(e) => write!(f, "task could not run: {e}"),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Taskwarrior {
    side: Side,
    program: String,
    env: Vec<(String, String)>,
}

impl Taskwarrior {
    pub(crate) fn for_side(side: Side, blocking: &Blocking) -> Self {
        let program = match &side {
            Side::Native => tools::program(Tool::Task),
            Side::Wsl(distro) => tools::wsl_in_job(Tool::Task, distro, blocking),
        };
        Self { side, program, env: Vec::new() }
    }

    pub(crate) fn with_env(mut self, key: &str, value: &str) -> Self {
        self.env.push((key.to_string(), value.to_string()));
        self
    }

    /// One attempt, no overrides: what `task` itself would do with `args`.
    fn spawn(&self, args: &[String], blocking: &Blocking) -> Result<Output, TaskError> {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let (program, argv) = self.side.command(&self.program, &refs);
        let mut cmd = hidden(program);
        cmd.args(argv).envs(self.env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        let output = match blocking.run_cancellable(&mut cmd) {
            Ok(output) => output,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(TaskError::Missing { program: self.program.clone() });
            },
            Err(e) => return Err(TaskError::Io(e.to_string())),
        };
        // A WSL login shell reports a missing program as 127, not ENOENT.
        if output.status.code() == Some(127) {
            return Err(TaskError::Missing { program: self.program.clone() });
        }
        Ok(output)
    }

    // Runs on a pool worker, where waiting out a busy lock is the point.
    #[allow(clippy::disallowed_methods)]
    fn run(&self, args: &[String], blocking: &Blocking) -> Result<Output, TaskError> {
        let mut argv: Vec<String> =
            UDA_DECLARATIONS.iter().map(|(k, v)| format!("rc.{k}={v}")).collect();
        argv.push("rc.confirmation=off".into());
        argv.extend(args.iter().cloned());
        let mut retries = BUSY_RETRIES.iter();
        loop {
            let output = self.spawn(&argv, blocking)?;
            if output.status.success() {
                return Ok(output);
            }
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            match retries.next() {
                Some(delay) if is_busy(&stderr) => std::thread::sleep(*delay),
                _ => return Err(TaskError::Failed { stderr }),
            }
        }
    }

    pub(crate) fn export(&self, filter: &[String], b: &Blocking) -> Result<Vec<Task>, TaskError> {
        let mut args = filter.to_vec();
        args.push("export".into());
        let output = self.run(&args, b)?;
        serde_json::from_slice(&output.stdout).map_err(|e| TaskError::Io(e.to_string()))
    }

    /// `--` stops taskwarrior reading `+tag` or `due:` out of the text.
    pub(crate) fn add(
        &self,
        project: &str,
        description: &str,
        subof: Option<&str>,
        order: i64,
        b: &Blocking,
    ) -> Result<String, TaskError> {
        let mut args = vec![
            "rc.verbose=new-uuid".to_string(),
            "add".into(),
            format!("project:{project}"),
            format!("order:{order}"),
        ];
        args.extend(subof.map(|s| format!("subof:{s}")));
        args.push("--".into());
        args.push(description.to_string());
        let output = self.run(&args, b)?;
        created_uuid(&String::from_utf8_lossy(&output.stdout))
            .ok_or_else(|| TaskError::Failed { stderr: "add printed no uuid".into() })
    }

    pub(crate) fn modify(&self, uuid: &str, mods: &[String], b: &Blocking) -> Result<(), TaskError> {
        let mut args = vec![uuid.to_string(), "modify".into()];
        args.extend(mods.iter().cloned());
        self.run(&args, b).map(drop)
    }

    pub(crate) fn describe(&self, uuid: &str, text: &str, b: &Blocking) -> Result<(), TaskError> {
        self.modify(uuid, &["--".into(), text.to_string()], b)
    }

    fn verb(&self, uuid: &str, verb: &str, b: &Blocking) -> Result<(), TaskError> {
        self.run(&[uuid.to_string(), verb.to_string()], b).map(drop)
    }

    pub(crate) fn done(&self, uuid: &str, b: &Blocking) -> Result<(), TaskError> {
        self.verb(uuid, "done", b)
    }

    pub(crate) fn undone(&self, uuid: &str, b: &Blocking) -> Result<(), TaskError> {
        self.modify(uuid, &["status:pending".into()], b)
    }

    pub(crate) fn start(&self, uuid: &str, b: &Blocking) -> Result<(), TaskError> {
        self.verb(uuid, "start", b)
    }

    pub(crate) fn stop(&self, uuid: &str, b: &Blocking) -> Result<(), TaskError> {
        self.verb(uuid, "stop", b)
    }

    pub(crate) fn delete(&self, uuid: &str, b: &Blocking) -> Result<(), TaskError> {
        self.verb(uuid, "delete", b)
    }

    /// Skips `run` on purpose: its overrides would make every declaration
    /// look present, and setup needs to know what the taskrc holds.
    pub(crate) fn rc_value(&self, key: &str, b: &Blocking) -> Result<Option<String>, TaskError> {
        let output = self.spawn(&["_get".to_string(), format!("rc.{key}")], b)?;
        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok((!value.is_empty()).then_some(value))
    }

    pub(crate) fn set_config(&self, key: &str, value: &str, b: &Blocking) -> Result<(), TaskError> {
        self.run(&["config".into(), key.to_string(), value.to_string()], b).map(drop)
    }
}

fn is_busy(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("database is locked") || lower.contains("sqlite_busy")
}

fn created_uuid(stdout: &str) -> Option<String> {
    stdout.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("Created task ")?.trim_end_matches('.');
        (rest.len() == 36 && rest.matches('-').count() == 4).then(|| rest.to_string())
    })
}
```

If `hidden(program)` complains that `String` is not `AsRef<OsStr>` by reference, pass `&program`. Drop the `#[allow]` on `run` if clippy does not disallow `std::thread::sleep`.

- [ ] **Step 4: Run to verify it passes**

Run: `devkit run task test --dir <worktree> -- tasks::taskwarrior`
Expected: 8 passed with `task` installed. If `describe_replaces_the_text_verbatim` fails because taskwarrior appends rather than replaces after `--`, switch `describe` to `modify(uuid, &[format!("description:{text}")])` and keep the test as written: it states the requirement.

- [ ] **Step 5: Commit**

```sh
git -C <worktree> add alacritree/src/tasks
git -C <worktree> commit -m "feat(tasks): run taskwarrior by uuid" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 4: Repository facts and the `task` CLI

**Files:**
- Create: `alacritree/src/tasks/facts.rs`
- Create: `alacritree/src/cli/task.rs`
- Modify: `alacritree/src/tasks/mod.rs` (`pub(crate) mod facts;`)
- Modify: `alacritree/src/cli/mod.rs` (`mod task;`, `Command::Task`, early return, `unreachable!` arm)
- Modify: `alacritree/src/cli/doctor.rs` (a UDA check per side)

**Interfaces:**
- Consumes: `scope::{Place, node, session_from_env}`, `Taskwarrior`, `UDA_DECLARATIONS`, `wsl::{classify, Location}`, `Side::command`, `Tool::Git`.
- Produces:
  - `facts::side_of(cwd: &Path) -> (Side, String)` (the side, and the cwd spelled on it)
  - `facts::place_from(porcelain: &str, toplevel: Option<&str>) -> Place`
  - `facts::place_for(cwd: &Path, &Blocking) -> (Side, Place)`
  - `cli::task::TaskCommand { Scope, Setup }`, `cli::task::run(TaskCommand, json: bool) -> i32`

- [ ] **Step 1: Write the failing tests** in `facts.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const LINKED: &str = "worktree /src/alacritree\nHEAD aaa\nbranch refs/heads/master\n\n\
        worktree /src/alacritree-worktrees/feat/x\nHEAD bbb\nbranch refs/heads/feat/x\n\n";

    #[test]
    fn the_main_worktree_names_the_repo_and_the_current_one_the_branch() {
        assert_eq!(
            place_from(LINKED, Some("/src/alacritree-worktrees/feat/x")),
            Place::Workspace { repo: "alacritree".into(), branch: "feat/x".into() }
        );
        assert_eq!(
            place_from(LINKED, Some("/src/alacritree")),
            Place::Workspace { repo: "alacritree".into(), branch: "master".into() }
        );
    }

    #[test]
    fn a_detached_head_falls_back_to_the_directory_name() {
        let porcelain = "worktree /src/r\nHEAD aaa\nbranch refs/heads/main\n\n\
            worktree /src/wt/review\nHEAD bbb\ndetached\n\n";
        assert_eq!(
            place_from(porcelain, Some("/src/wt/review")),
            Place::Workspace { repo: "r".into(), branch: "review".into() }
        );
    }

    #[test]
    fn a_branch_forced_into_two_worktrees_falls_back_to_the_directory_name() {
        let porcelain = "worktree /src/r\nHEAD aaa\nbranch refs/heads/main\n\n\
            worktree /src/wt/copy\nHEAD aaa\nbranch refs/heads/main\n\n";
        assert_eq!(
            place_from(porcelain, Some("/src/wt/copy")),
            Place::Workspace { repo: "r".into(), branch: "copy".into() }
        );
    }

    #[test]
    fn a_bare_repo_drops_its_git_suffix_and_has_no_workspace_at_its_root() {
        let porcelain = "worktree /src/proj.git\nbare\n\n\
            worktree /src/proj-main\nHEAD aaa\nbranch refs/heads/main\n\n";
        assert_eq!(place_from(porcelain, None), Place::Project { repo: "proj".into() });
        assert_eq!(
            place_from(porcelain, Some("/src/proj-main")),
            Place::Workspace { repo: "proj".into(), branch: "main".into() }
        );
    }

    #[test]
    fn a_submodule_is_named_by_its_own_top_level() {
        let porcelain = "worktree /src/super/vendor/lib\nHEAD aaa\nbranch refs/heads/main\n\n";
        assert_eq!(
            place_from(porcelain, Some("/src/super/vendor/lib")),
            Place::Workspace { repo: "lib".into(), branch: "main".into() }
        );
    }

    #[test]
    fn windows_separators_and_trailing_slashes_still_match() {
        let porcelain = "worktree C:/src/r\nHEAD a\nbranch refs/heads/main\n\n";
        assert_eq!(
            place_from(porcelain, Some("C:\\src\\r\\")),
            Place::Workspace { repo: "r".into(), branch: "main".into() }
        );
    }

    #[test]
    fn nothing_to_parse_is_global() {
        assert_eq!(place_from("", Some("/x")), Place::Global);
    }

    #[cfg(windows)]
    #[test]
    fn a_distro_path_runs_on_that_distro() {
        let (side, cwd) = side_of(Path::new(r"\\wsl.localhost\Ubuntu\home\lev\r"));
        assert_eq!(side, Side::Wsl("Ubuntu".into()));
        assert_eq!(cwd, "/home/lev/r");
    }

    #[test]
    fn a_real_repository_reports_its_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("myrepo");
        std::fs::create_dir(&repo).unwrap();
        // A test has no UI thread for a blocking wait to stall.
        #[allow(clippy::disallowed_methods)]
        let init = hidden("git").args(["init", "-q", "-b", "trunk"]).current_dir(&repo).status();
        if !init.is_ok_and(|s| s.success()) {
            return;
        }
        let (_, place) = crate::jobs::on_this_thread(|b| place_for(&repo, b));
        assert_eq!(place, Place::Workspace { repo: "myrepo".into(), branch: "trunk".into() });
        let (_, outside) = crate::jobs::on_this_thread(|b| place_for(dir.path(), b));
        assert_eq!(outside, Place::Global);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `devkit run task test --dir <worktree> -- tasks::facts`
Expected: FAIL to compile, `place_from` not found.

- [ ] **Step 3: Implement** `facts.rs` above the tests:

```rust
//! Repository facts for a working directory, read by `git` on the side the
//! directory lives on.  A Windows binary started from inside WSL sees its cwd
//! as a `\\wsl.localhost` path, and only the distro's own git reads that
//! checkout correctly.

use std::path::Path;

use crate::command_ext::hidden;
use crate::jobs::Blocking;
use crate::multiplexer::Side;
use crate::tasks::scope::Place;
use crate::tools::{self, Tool};
use crate::wsl;

pub(crate) fn side_of(cwd: &Path) -> (Side, String) {
    match wsl::classify(cwd) {
        wsl::Location::Wsl { distro, linux_path } => (Side::Wsl(distro), linux_path),
        wsl::Location::Windows(path) => (Side::Native, path.display().to_string()),
    }
}

pub(crate) fn place_for(cwd: &Path, b: &Blocking) -> (Side, Place) {
    let (side, dir) = side_of(cwd);
    let git = match &side {
        Side::Native => tools::program(Tool::Git),
        Side::Wsl(distro) => tools::wsl_in_job(Tool::Git, distro, b),
    };
    let run = |args: &[&str]| -> Option<String> {
        let mut argv = vec!["-C", dir.as_str()];
        argv.extend_from_slice(args);
        let (program, argv) = side.command(&git, &argv);
        let output = b.run_cancellable(hidden(program).args(argv)).ok()?;
        output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    };
    let Some(porcelain) = run(&["worktree", "list", "--porcelain"]) else {
        return (side, Place::Global);
    };
    let toplevel = run(&["rev-parse", "--show-toplevel"]);
    let place = place_from(&porcelain, toplevel.as_deref().map(str::trim));
    (side, place)
}

struct Entry {
    path: String,
    bare: bool,
    branch: Option<String>,
}

fn entries(porcelain: &str) -> Vec<Entry> {
    porcelain
        .replace("\r\n", "\n")
        .split("\n\n")
        .filter_map(|block| {
            let mut lines = block.lines();
            let path = lines.next()?.strip_prefix("worktree ")?.to_string();
            let mut entry = Entry { path, bare: false, branch: None };
            for line in lines {
                if line == "bare" {
                    entry.bare = true;
                } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
                    entry.branch = Some(branch.to_string());
                }
            }
            Some(entry)
        })
        .collect()
}

fn normalize(path: &str) -> String {
    path.replace('\\', "/").trim_end_matches('/').to_string()
}

fn basename(path: &str) -> String {
    normalize(path).rsplit('/').next().unwrap_or_default().to_string()
}

/// `git worktree list` puts the main worktree first, and the main worktree is
/// what names the repository for all of them.
pub(crate) fn place_from(porcelain: &str, toplevel: Option<&str>) -> Place {
    let all = entries(porcelain);
    let Some(main) = all.first() else { return Place::Global };
    let name = basename(&main.path);
    let repo = match name.strip_suffix(".git") {
        Some(stripped) if main.bare => stripped.to_string(),
        _ => name,
    };
    let current = toplevel.map(normalize).and_then(|top| {
        all.iter().find(|e| !e.bare && normalize(&e.path) == top)
    });
    let Some(current) = current else { return Place::Project { repo } };
    let shared = |branch: &String| all.iter().filter(|e| e.branch.as_ref() == Some(branch)).count() > 1;
    let branch = match &current.branch {
        Some(branch) if !shared(branch) => branch.clone(),
        _ => basename(&current.path),
    };
    Place::Workspace { repo, branch }
}
```

If `Side` lacks `PartialEq` or `Debug` (check `multiplexer/mod.rs:40`), add the derives.

- [ ] **Step 4: Run to verify facts pass**

Run: `devkit run task test --dir <worktree> -- tasks::facts`
Expected: 9 passed on Windows, 8 elsewhere.

- [ ] **Step 5: Add the CLI.** In `cli/mod.rs`, declare `mod task;` with the other modules and add to `Command`:

```rust
    /// Task lists kept in taskwarrior.  Runs without a window.
    Task {
        #[command(subcommand)]
        command: task::TaskCommand,
    },
```

next to the `Command::Doctor` early return:

```rust
        // Reads git and taskwarrior directly, so it answers in a bare herdr
        // pane with no alacritree running.
        Command::Task { command } => return Some(task::run(command, cli.json)),
```

and add `| Command::Task { .. }` to the `unreachable!("handled before dispatch")` arm.

`cli/task.rs`:

```rust
//! `alacritree task`: the project an agent's shell writes to, and the
//! one-time UDA declarations agents calling `task` directly depend on.

use clap::Subcommand;

use crate::jobs;
use crate::multiplexer::Side;
use crate::tasks::facts;
use crate::tasks::scope::{node, session_from_env};
use crate::tasks::taskwarrior::{TaskError, Taskwarrior, UDA_DECLARATIONS};

#[derive(Debug, Subcommand)]
pub(super) enum TaskCommand {
    /// Print the taskwarrior project for this directory and agent session.
    Scope,
    /// Declare the `subof` and `order` UDAs in the taskrc on every side.
    Setup,
}

pub(super) fn run(command: TaskCommand, json: bool) -> i32 {
    match command {
        TaskCommand::Scope => scope(json),
        TaskCommand::Setup => setup(json),
    }
}

fn scope(json: bool) -> i32 {
    let Ok(cwd) = std::env::current_dir() else {
        eprintln!("alacritree: the current directory is unreadable");
        return 1;
    };
    let (_, place) = jobs::on_this_thread(|b| facts::place_for(&cwd, b));
    let project = node(&place, session_from_env(|key| std::env::var(key).ok()).as_ref());
    if json {
        println!("{}", serde_json::json!({ "project": project }));
    } else {
        println!("{project}");
    }
    0
}

fn declare(side: Side) -> Result<Vec<&'static str>, TaskError> {
    jobs::on_this_thread(|b| {
        let tw = Taskwarrior::for_side(side, b);
        let mut written = Vec::new();
        for (key, value) in UDA_DECLARATIONS {
            if tw.rc_value(key, b)?.as_deref() != Some(value) {
                tw.set_config(key, value, b)?;
                written.push(key);
            }
        }
        Ok(written)
    })
}

fn setup(json: bool) -> i32 {
    let mut sides = vec![Side::Native];
    sides.extend(installed_distros().into_iter().map(Side::Wsl));
    let mut failed = false;
    for side in sides {
        let name = side.name();
        let result = declare(side);
        failed |= result.is_err();
        match (json, result) {
            (true, result) => {
                let (written, error) = match result {
                    Ok(written) => (written, None),
                    Err(e) => (Vec::new(), Some(e.to_string())),
                };
                println!("{}", serde_json::json!({ "side": name, "written": written, "error": error }));
            },
            (false, Ok(written)) if written.is_empty() => println!("{name}: already declared"),
            (false, Ok(written)) => println!("{name}: declared {}", written.join(", ")),
            (false, Err(e)) => eprintln!("{name}: {e}"),
        }
    }
    i32::from(failed)
}
```

`installed_distros()` is whatever `doctor.rs` calls to list distros (its probes at `doctor.rs:251-305`); call that function directly rather than adding a wrapper. A side with no `task` prints `taskwarrior not found` and sets the exit code, which is intended.

In `doctor.rs`, beside the per-distro tool probes, add a `taskwarrior UDAs` check for each side when `config.integrations.taskwarrior.enabled`: `Ok` when `rc_value("uda.subof.type")` is `Some("uuid")` and `rc_value("uda.order.type")` is `Some("numeric")`, `Warn` with `run alacritree task setup` otherwise, and `Warn` naming the error when `task` is missing. Keep the decision in a pure function, `fn uda_check(side: &str, subof: Option<&str>, order: Option<&str>) -> Check`, and test it the way `wsl_distro_check` is tested at `doctor.rs:863`:

```rust
#[test]
fn uda_check_warns_until_both_are_declared() {
    assert_eq!(uda_check("native", Some("uuid"), Some("numeric")).status, Status::Ok);
    assert_eq!(uda_check("native", Some("uuid"), None).status, Status::Warn);
    assert_eq!(uda_check("wsl:Ubuntu", None, None).status, Status::Warn);
}
```

Use the real name of doctor's check struct in place of `Check`.

- [ ] **Step 6: Run everything**

Run: `devkit run task test --dir <worktree>`, then from a Claude Code Bash tool inside the worktree: `cargo run -p alacritree -- task scope`
Expected: all tests pass, and the manual run prints `alacritree.feat-86-taskwarrior-tasks.claude-<id>`.

- [ ] **Step 7: Commit**

```sh
git -C <worktree> add alacritree/src
git -C <worktree> commit -m "feat(cli): add task scope and task setup" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 5: The tree model

**Files:**
- Create: `alacritree/src/tasks/tree.rs`
- Modify: `alacritree/src/tasks/mod.rs` (`pub(crate) mod tree;`)

**Interfaces:**
- Consumes: `taskwarrior::{Task, Status}`, `scope::GLOBAL`.
- Produces:
  - `STRIDE: i64 = 1024`
  - `Row { uuid: String, depth: usize, text: String, status: Status, started: bool }`
  - `SectionKind { Global, Project, Workspace, Session }`
  - `Section { node: String, kind: SectionKind, rows: Vec<Row> }`
  - `sections(tasks: &[Task], repo: Option<&str>, workspace: Option<&str>) -> Vec<Section>`
  - `rows(tasks: &[&Task]) -> Vec<Row>`
  - `Edit { Add { project: String, description: String, subof: Option<String>, order: i64 }, Modify { uuid: String, mods: Vec<String> } }`
  - `insert_after(tasks: &[&Task], project: &str, after: Option<&str>, description: &str) -> Vec<Edit>` (renumbers first when needed, `Add` last)
  - `indent(tasks: &[&Task], uuid: &str) -> Vec<Edit>`
  - `dedent(tasks: &[&Task], uuid: &str) -> Vec<Edit>`
  - The `tasks` given to `insert_after`, `indent` and `dedent` are one section's tasks.

- [ ] **Step 1: Write the failing tests** in `tree.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn task(uuid: &str, project: &str, subof: Option<&str>, order: Option<i64>) -> Task {
        Task {
            uuid: uuid.into(),
            description: uuid.into(),
            status: Status::Pending,
            start: None,
            subof: subof.map(Into::into),
            order,
            project: Some(project.into()),
            entry: Some(format!("20260921T00000{}Z", uuid.len())),
            modified: None,
        }
    }

    fn shape(rows: &[Row]) -> Vec<(&str, usize)> {
        rows.iter().map(|r| (r.uuid.as_str(), r.depth)).collect()
    }

    fn refs(tasks: &[Task]) -> Vec<&Task> {
        tasks.iter().collect()
    }

    #[test]
    fn children_follow_their_parent_in_order() {
        let t = [
            task("b", "r", None, Some(2048)),
            task("a", "r", None, Some(1024)),
            task("a2", "r", Some("a"), Some(2048)),
            task("a1", "r", Some("a"), Some(1024)),
        ];
        assert_eq!(shape(&rows(&refs(&t))), [("a", 0), ("a1", 1), ("a2", 1), ("b", 0)]);
    }

    #[test]
    fn orphan_subof_renders_as_root() {
        let t = [task("x", "r", Some("gone"), Some(1024))];
        assert_eq!(shape(&rows(&refs(&t))), [("x", 0)]);
    }

    #[test]
    fn subof_cycle_terminates() {
        let t = [task("a", "r", Some("b"), Some(1)), task("b", "r", Some("a"), Some(2))];
        let got = rows(&refs(&t));
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn unordered_tasks_sort_after_ordered_ones() {
        let t = [task("late", "r", None, None), task("first", "r", None, Some(1024))];
        assert_eq!(shape(&rows(&refs(&t))), [("first", 0), ("late", 0)]);
    }

    #[test]
    fn sections_split_by_node() {
        let t = [
            task("g", "global", None, None),
            task("p", "r", None, None),
            task("w", "r.main", None, None),
            task("s2", "r.main.codex-2", None, None),
            task("s1", "r.main.claude-1", None, None),
            task("other", "r.feat", None, None),
        ];
        let got = sections(&t, Some("r"), Some("r.main"));
        let kinds: Vec<(&str, SectionKind)> = got.iter().map(|s| (s.node.as_str(), s.kind)).collect();
        assert_eq!(kinds[..3], [
            ("global", SectionKind::Global),
            ("r", SectionKind::Project),
            ("r.main", SectionKind::Workspace)
        ]);
        assert_eq!(kinds.len(), 5, "r.feat belongs to another workspace");
        assert!(kinds[3..].iter().all(|(_, k)| *k == SectionKind::Session));
    }

    #[test]
    fn the_home_tab_shows_only_global() {
        let t = [task("g", "global", None, None), task("p", "r", None, None)];
        let got = sections(&t, None, None);
        assert_eq!(got.iter().map(|s| s.kind).collect::<Vec<_>>(), [SectionKind::Global]);
    }

    #[test]
    fn empty_scopes_still_get_a_section_to_type_into() {
        let got = sections(&[], Some("r"), Some("r.main"));
        let kinds: Vec<SectionKind> = got.iter().map(|s| s.kind).collect();
        assert_eq!(kinds, [SectionKind::Global, SectionKind::Project, SectionKind::Workspace]);
    }

    #[test]
    fn insert_takes_the_midpoint_of_the_gap() {
        let t = [task("a", "r", None, Some(1024)), task("b", "r", None, Some(2048))];
        assert_eq!(insert_after(&refs(&t), "r", Some("a"), "new"), [Edit::Add {
            project: "r".into(),
            description: "new".into(),
            subof: None,
            order: 1536
        }]);
    }

    #[test]
    fn insert_into_an_empty_section_starts_at_one_stride() {
        let Some(Edit::Add { order, .. }) = insert_after(&[], "r", None, "x").pop() else { panic!() };
        assert_eq!(order, STRIDE);
    }

    #[test]
    fn insert_at_the_end_adds_a_stride() {
        let t = [task("a", "r", None, Some(1024))];
        let Some(Edit::Add { order, .. }) = insert_after(&refs(&t), "r", Some("a"), "x").pop() else {
            panic!()
        };
        assert_eq!(order, 2048);
    }

    #[test]
    fn a_closed_gap_renumbers_the_siblings_first() {
        let t = [task("a", "r", None, Some(10)), task("b", "r", None, Some(11))];
        let edits = insert_after(&refs(&t), "r", Some("a"), "x");
        assert_eq!(edits[..2], [
            Edit::Modify { uuid: "a".into(), mods: vec!["order:1024".into()] },
            Edit::Modify { uuid: "b".into(), mods: vec!["order:2048".into()] },
        ]);
        let Edit::Add { order, .. } = &edits[2] else { panic!() };
        assert_eq!(*order, 1536);
    }

    #[test]
    fn insert_below_a_child_stays_a_sibling_of_that_child() {
        let t = [task("a", "r", None, Some(1024)), task("a1", "r", Some("a"), Some(1024))];
        let Some(Edit::Add { subof, .. }) = insert_after(&refs(&t), "r", Some("a1"), "x").pop() else {
            panic!()
        };
        assert_eq!(subof.as_deref(), Some("a"));
    }

    #[test]
    fn indent_moves_under_the_previous_sibling_after_its_children() {
        let t = [
            task("a", "r", None, Some(1024)),
            task("a1", "r", Some("a"), Some(1024)),
            task("b", "r", None, Some(2048)),
        ];
        assert_eq!(indent(&refs(&t), "b"), [Edit::Modify {
            uuid: "b".into(),
            mods: vec!["subof:a".into(), "order:2048".into()]
        }]);
    }

    #[test]
    fn indent_without_a_previous_sibling_does_nothing() {
        let t = [task("a", "r", None, Some(1024))];
        assert!(indent(&refs(&t), "a").is_empty());
    }

    #[test]
    fn dedent_lands_right_after_the_old_parent() {
        let t = [
            task("a", "r", None, Some(1024)),
            task("a1", "r", Some("a"), Some(1024)),
            task("b", "r", None, Some(2048)),
        ];
        assert_eq!(dedent(&refs(&t), "a1"), [Edit::Modify {
            uuid: "a1".into(),
            mods: vec!["subof:".into(), "order:1536".into()]
        }]);
    }

    #[test]
    fn dedent_at_the_root_does_nothing() {
        let t = [task("a", "r", None, Some(1024))];
        assert!(dedent(&refs(&t), "a").is_empty());
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `devkit run task test --dir <worktree> -- tasks::tree`
Expected: FAIL to compile.

- [ ] **Step 3: Implement** `tree.rs` above the tests:

```rust
//! The tab's model: tasks split into scope sections, nested by `subof`,
//! ordered by `order`, and the edits that inserting or indenting turns into.
//! Free of egui so it can be tested without a frame.

use std::collections::{HashMap, HashSet};

use crate::tasks::scope::GLOBAL;
use crate::tasks::taskwarrior::{Status, Task};

pub(crate) const STRIDE: i64 = 1024;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Row {
    pub uuid: String,
    pub depth: usize,
    pub text: String,
    pub status: Status,
    pub started: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SectionKind {
    Global,
    Project,
    Workspace,
    Session,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Section {
    pub node: String,
    pub kind: SectionKind,
    pub rows: Vec<Row>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Edit {
    Add { project: String, description: String, subof: Option<String>, order: i64 },
    Modify { uuid: String, mods: Vec<String> },
}

fn project(t: &Task) -> &str {
    t.project.as_deref().unwrap_or(GLOBAL)
}

/// Agents may add tasks with no `order`; those sort after ordered ones,
/// oldest first, so a new agent task lands at the bottom.
fn sort_key(t: &Task) -> (bool, i64, String) {
    (t.order.is_none(), t.order.unwrap_or(0), t.entry.clone().unwrap_or_default())
}

/// A `subof` naming a task outside `tasks` is treated as none, so a task
/// whose parent was deleted or lives in another scope still shows.
fn parent_of<'a>(t: &'a Task, present: &HashSet<&str>) -> Option<&'a str> {
    t.subof.as_deref().filter(|p| present.contains(p) && *p != t.uuid)
}

fn present<'a>(tasks: &[&'a Task]) -> HashSet<&'a str> {
    tasks.iter().map(|t| t.uuid.as_str()).collect()
}

pub(crate) fn sections(tasks: &[Task], repo: Option<&str>, workspace: Option<&str>) -> Vec<Section> {
    let in_node = |node: &str| -> Vec<&Task> { tasks.iter().filter(|t| project(t) == node).collect() };
    let section =
        |node: &str, kind| Section { node: node.to_string(), kind, rows: rows(&in_node(node)) };
    let mut out = vec![section(GLOBAL, SectionKind::Global)];
    out.extend(repo.map(|repo| section(repo, SectionKind::Project)));
    if let Some(workspace) = workspace {
        out.push(section(workspace, SectionKind::Workspace));
        let prefix = format!("{workspace}.");
        let mut sessions: Vec<&str> = tasks
            .iter()
            .map(project)
            .filter(|p| p.starts_with(&prefix))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let latest =
            |node: &str| in_node(node).iter().filter_map(|t| t.modified.clone()).max().unwrap_or_default();
        sessions.sort_by_key(|node| (std::cmp::Reverse(latest(node)), node.to_string()));
        out.extend(sessions.into_iter().map(|node| section(node, SectionKind::Session)));
    }
    out
}

pub(crate) fn rows(tasks: &[&Task]) -> Vec<Row> {
    let present = present(tasks);
    let mut children: HashMap<Option<&str>, Vec<&Task>> = HashMap::new();
    for t in tasks {
        children.entry(parent_of(t, &present)).or_default().push(t);
    }
    for list in children.values_mut() {
        list.sort_by_key(|t| sort_key(t));
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    walk(&children, None, 0, &mut seen, &mut out);
    // Tasks in a `subof` cycle have no root to be reached from.
    let mut stranded: Vec<&Task> =
        tasks.iter().copied().filter(|t| !seen.contains(t.uuid.as_str())).collect();
    stranded.sort_by_key(|t| sort_key(t));
    for t in stranded {
        if seen.insert(t.uuid.clone()) {
            out.push(row(t, 0));
            walk(&children, Some(&t.uuid), 1, &mut seen, &mut out);
        }
    }
    out
}

fn walk(
    children: &HashMap<Option<&str>, Vec<&Task>>,
    parent: Option<&str>,
    depth: usize,
    seen: &mut HashSet<String>,
    out: &mut Vec<Row>,
) {
    for t in children.get(&parent).into_iter().flatten() {
        if seen.insert(t.uuid.clone()) {
            out.push(row(t, depth));
            walk(children, Some(&t.uuid), depth + 1, seen, out);
        }
    }
}

fn row(t: &Task, depth: usize) -> Row {
    Row {
        uuid: t.uuid.clone(),
        depth,
        text: t.description.clone(),
        status: t.status,
        started: t.start.is_some(),
    }
}

fn siblings<'a>(tasks: &[&'a Task], parent: Option<&str>) -> Vec<&'a Task> {
    let present = present(tasks);
    let mut list: Vec<&Task> =
        tasks.iter().copied().filter(|t| parent_of(t, &present) == parent).collect();
    list.sort_by_key(|t| sort_key(t));
    list
}

fn order_mod(order: i64) -> String {
    format!("order:{order}")
}

/// The `order` for a new sibling placed after `list[index]`, or first when
/// `index` is `None`.  When no integer fits between the neighbours, the whole
/// set is renumbered at the stride first.
fn slot(list: &[&Task], index: Option<usize>) -> (Vec<Edit>, i64) {
    let at = |i: usize| list[i].order.unwrap_or((i as i64 + 1) * STRIDE);
    let low = index.map_or(0, at);
    let next = index.map_or(0, |i| i + 1);
    let high = (next < list.len()).then(|| at(next));
    match high {
        None => (Vec::new(), low + STRIDE),
        Some(high) if high - low >= 2 => (Vec::new(), low + (high - low) / 2),
        Some(_) => {
            let renumber = list
                .iter()
                .enumerate()
                .map(|(i, t)| Edit::Modify {
                    uuid: t.uuid.clone(),
                    mods: vec![order_mod((i as i64 + 1) * STRIDE)],
                })
                .collect();
            let low = index.map_or(0, |i| (i as i64 + 1) * STRIDE);
            (renumber, low + STRIDE / 2)
        },
    }
}

pub(crate) fn insert_after(
    tasks: &[&Task],
    project: &str,
    after: Option<&str>,
    description: &str,
) -> Vec<Edit> {
    let present = present(tasks);
    let anchor = after.and_then(|u| tasks.iter().copied().find(|t| t.uuid == u));
    let parent = anchor.and_then(|t| parent_of(t, &present));
    let list = siblings(tasks, parent);
    let index = anchor.and_then(|a| list.iter().position(|t| t.uuid == a.uuid));
    let (mut edits, order) = slot(&list, index);
    edits.push(Edit::Add {
        project: project.to_string(),
        description: description.to_string(),
        subof: parent.map(str::to_string),
        order,
    });
    edits
}

pub(crate) fn indent(tasks: &[&Task], uuid: &str) -> Vec<Edit> {
    let present = present(tasks);
    let Some(me) = tasks.iter().copied().find(|t| t.uuid == uuid) else { return Vec::new() };
    let list = siblings(tasks, parent_of(me, &present));
    let Some(pos) = list.iter().position(|t| t.uuid == uuid) else { return Vec::new() };
    let Some(new_parent) = pos.checked_sub(1).map(|i| list[i]) else { return Vec::new() };
    let children = siblings(tasks, Some(&new_parent.uuid));
    let (mut edits, order) = slot(&children, children.len().checked_sub(1));
    edits.push(Edit::Modify {
        uuid: uuid.to_string(),
        mods: vec![format!("subof:{}", new_parent.uuid), order_mod(order)],
    });
    edits
}

pub(crate) fn dedent(tasks: &[&Task], uuid: &str) -> Vec<Edit> {
    let present = present(tasks);
    let Some(me) = tasks.iter().copied().find(|t| t.uuid == uuid) else { return Vec::new() };
    let Some(parent) = parent_of(me, &present).and_then(|p| tasks.iter().copied().find(|t| t.uuid == p))
    else {
        return Vec::new();
    };
    let grandparent = parent_of(parent, &present);
    let list = siblings(tasks, grandparent);
    let index = list.iter().position(|t| t.uuid == parent.uuid);
    let (mut edits, order) = slot(&list, index);
    edits.push(Edit::Modify {
        uuid: uuid.to_string(),
        mods: vec![format!("subof:{}", grandparent.unwrap_or("")), order_mod(order)],
    });
    edits
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `devkit run task test --dir <worktree> -- tasks::tree`
Expected: 16 passed.

- [ ] **Step 5: Commit**

```sh
git -C <worktree> add alacritree/src/tasks
git -C <worktree> commit -m "feat(tasks): model task trees and their edits" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 6: `alacritree hook`

**Files:**
- Create: `alacritree/src/tasks/hook.rs`
- Modify: `alacritree/src/tasks/mod.rs` (`pub(crate) mod hook;`)
- Modify: `alacritree/src/cli/mod.rs` (`Command::Hook`, early return, `unreachable!` arm)
- Create: `alacritree/tests/hook.rs`
- Create: `alacritree/tests/fixtures/hook/claude-session-start.json`, `codex-session-start.json`, `claude-user-prompt-submit.json`, `codex-user-prompt-submit.json`

**Interfaces:**
- Consumes: `facts::place_for`, `scope::{node, sanitize, Harness, SessionRef, Place, GLOBAL}`, `Taskwarrior`, `tree::rows`, `digest::stable_digest`, `state::config_dir`.
- Produces:
  - `hook::Event { SessionStart, UserPromptSubmit }` (clap `ValueEnum`: `session-start`, `user-prompt-submit`)
  - `hook::context(&Place, Option<&SessionRef>, &[Task]) -> String`
  - `hook::output(Event, &str) -> String`
  - `hook::run(Event, Harness, stdin: &str, state_dir: Option<&Path>) -> Option<String>`

- [ ] **Step 1: Write the fixtures.** They mirror what each harness sends; the hook reads only `session_id` and `cwd`.

`claude-session-start.json`:

```json
{"session_id":"k-123","transcript_path":"/tmp/t.jsonl","cwd":"CWD","hook_event_name":"SessionStart","source":"startup"}
```

`codex-session-start.json`:

```json
{"session_id":"0199a-root","transcript_path":null,"cwd":"CWD","hook_event_name":"SessionStart","model":"gpt-5","permission_mode":"default","source":"resume"}
```

`claude-user-prompt-submit.json`:

```json
{"session_id":"k-123","transcript_path":"/tmp/t.jsonl","cwd":"CWD","hook_event_name":"UserPromptSubmit","prompt":"go"}
```

`codex-user-prompt-submit.json`:

```json
{"session_id":"0199a-root","transcript_path":null,"cwd":"CWD","hook_event_name":"UserPromptSubmit","model":"gpt-5","permission_mode":"default","prompt":"go","turn_id":"t1"}
```

Before relying on them, compare the codex fixtures with `SessionStartCommandInput` and `UserPromptSubmitCommandInput` in `docm path codex`'s `codex-rs/hooks/src/schema.rs`, and fix any field name that differs.

- [ ] **Step 2: Write the failing unit tests** at the bottom of `hook.rs`. They exercise the pure parts only, so they never touch a real taskwarrior:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn task(uuid: &str, project: &str, text: &str, status: Status) -> Task {
        Task {
            uuid: uuid.into(),
            description: text.into(),
            status,
            start: None,
            subof: None,
            order: Some(1024),
            project: Some(project.into()),
            entry: None,
            modified: None,
        }
    }

    #[test]
    fn context_lists_each_visible_scope_and_names_the_write_target() {
        let place = Place::Workspace { repo: "r".into(), branch: "main".into() };
        let me = SessionRef { harness: Harness::Codex, id: "s1".into() };
        let tasks = [
            task("11111111-aaaa", "global", "global chore", Status::Pending),
            task("22222222-bbbb", "r.main", "ship it", Status::Completed),
            task("33333333-cccc", "r.main.codex-s1", "my step", Status::Pending),
            task("44444444-dddd", "r.main.claude-other", "not mine", Status::Pending),
        ];
        let text = context(&place, Some(&me), &tasks);
        assert!(text.contains("`r.main.codex-s1`"));
        assert!(text.contains("- [ ] my step (33333333)"));
        assert!(text.contains("- [x] ship it (22222222)"));
        assert!(text.contains("global chore"));
        assert!(!text.contains("not mine"), "other agents' lists stay out");
        assert!(text.contains("subof:"));
    }

    #[test]
    fn without_a_session_the_workspace_is_the_write_target() {
        let place = Place::Workspace { repo: "r".into(), branch: "main".into() };
        assert!(context(&place, None, &[]).contains("`r.main`"));
    }

    #[test]
    fn output_is_one_json_object_for_the_event() {
        let json: serde_json::Value =
            serde_json::from_str(&output(Event::UserPromptSubmit, "ctx")).unwrap();
        assert_eq!(json["hookSpecificOutput"]["hookEventName"], "UserPromptSubmit");
        assert_eq!(json["hookSpecificOutput"]["additionalContext"], "ctx");
        assert_eq!(json.as_object().unwrap().len(), 1);
    }
}
```

- [ ] **Step 3: Write the failing process tests** in `tests/hook.rs`:

```rust
//! `alacritree hook` as a harness runs it: a fresh process, a payload on
//! stdin, JSON or nothing on stdout, and exit 0 whatever happens.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

use alacritree::command_ext::hidden;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_alacritree")
}

fn fixture(name: &str, cwd: &Path) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hook").join(name);
    let raw = std::fs::read_to_string(path).unwrap();
    raw.replace("\"CWD\"", &serde_json::to_string(cwd.to_str().unwrap()).unwrap())
}

/// A private taskwarrior, config and state dir, so nothing the developer
/// owns is read or written.
fn hook(home: &Path, args: &[&str], stdin: &str) -> Output {
    let rc = home.join("taskrc");
    std::fs::write(&rc, "").unwrap();
    // A test has no UI thread for a blocking wait to stall.
    #[allow(clippy::disallowed_methods)]
    let mut child = hidden(binary())
        .args(args)
        .env("TASKRC", &rc)
        .env("TASKDATA", home.join("data"))
        .env("XDG_CONFIG_HOME", home)
        .env("XDG_STATE_HOME", home)
        .env("APPDATA", home)
        .env("LOCALAPPDATA", home)
        .env("HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    #[allow(clippy::disallowed_methods)]
    child.wait_with_output().unwrap()
}

fn task_installed() -> bool {
    #[allow(clippy::disallowed_methods)]
    hidden("task").arg("--version").output().is_ok_and(|o| o.status.success())
}

fn repo(home: &Path) -> PathBuf {
    let repo = home.join("myrepo");
    std::fs::create_dir(&repo).unwrap();
    #[allow(clippy::disallowed_methods)]
    hidden("git").args(["init", "-q", "-b", "main"]).current_dir(&repo).status().unwrap();
    repo
}

#[test]
fn session_start_prints_one_json_object_for_both_harnesses() {
    if !task_installed() {
        return;
    }
    for (harness, file) in
        [("claude", "claude-session-start.json"), ("codex", "codex-session-start.json")]
    {
        let home = tempfile::tempdir().unwrap();
        let cwd = repo(home.path());
        let args = ["hook", "session-start", "--harness", harness];
        let out = hook(home.path(), &args, &fixture(file, &cwd));
        assert!(out.status.success());
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
        assert_eq!(json["hookSpecificOutput"]["hookEventName"], "SessionStart");
        let ctx = json["hookSpecificOutput"]["additionalContext"].as_str().unwrap();
        assert!(ctx.contains(&format!("myrepo.main.{harness}-")), "{ctx}");
    }
}

#[test]
fn an_unchanged_list_is_injected_once() {
    if !task_installed() {
        return;
    }
    for (harness, file) in
        [("claude", "claude-user-prompt-submit.json"), ("codex", "codex-user-prompt-submit.json")]
    {
        let home = tempfile::tempdir().unwrap();
        let cwd = repo(home.path());
        let args = ["hook", "user-prompt-submit", "--harness", harness];
        let first = hook(home.path(), &args, &fixture(file, &cwd));
        assert!(!first.stdout.is_empty(), "the first prompt sees the list");
        let second = hook(home.path(), &args, &fixture(file, &cwd));
        assert!(second.status.success());
        assert!(second.stdout.is_empty(), "an unchanged list is not repeated");
    }
}

#[test]
fn a_missing_task_binary_prints_nothing_and_exits_zero() {
    let home = tempfile::tempdir().unwrap();
    let cwd = repo(home.path());
    let args = [
        "-o",
        "integrations.taskwarrior.path='alacritree-no-such-task-binary'",
        "hook",
        "session-start",
        "--harness",
        "claude",
    ];
    let out = hook(home.path(), &args, &fixture("claude-session-start.json", &cwd));
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
}

#[test]
fn garbage_stdin_exits_zero() {
    let home = tempfile::tempdir().unwrap();
    for stdin in ["", "not json", "{}", "[1,2]"] {
        let out = hook(home.path(), &["hook", "session-start", "--harness", "codex"], stdin);
        assert!(out.status.success(), "{stdin:?}");
        if !out.stdout.is_empty() {
            serde_json::from_slice::<serde_json::Value>(&out.stdout).expect("JSON when printed");
        }
    }
}
```

- [ ] **Step 4: Run to verify they fail**

Run: `devkit run task test --dir <worktree> -- hook`
Expected: FAIL, `tasks::hook` does not exist and `hook` is not a subcommand.

- [ ] **Step 5: Implement** `hook.rs` above the tests:

```rust
//! `alacritree hook <event>`: the one command a harness's hook config calls.
//! It never blocks a turn.  Every failure prints nothing and exits 0, and
//! the only thing it prints is one JSON object the harness adds to the
//! model's context.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::digest::stable_digest;
use crate::jobs;
use crate::tasks::facts;
use crate::tasks::scope::{GLOBAL, Harness, Place, SessionRef, node, sanitize};
use crate::tasks::taskwarrior::{Status, Task, Taskwarrior};
use crate::tasks::tree;

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum Event {
    SessionStart,
    UserPromptSubmit,
}

impl Event {
    fn wire_name(self) -> &'static str {
        match self {
            Self::SessionStart => "SessionStart",
            Self::UserPromptSubmit => "UserPromptSubmit",
        }
    }
}

#[derive(Deserialize, Default)]
struct Payload {
    session_id: Option<String>,
    cwd: Option<PathBuf>,
}

pub(crate) fn output(event: Event, context: &str) -> String {
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": event.wire_name(),
            "additionalContext": context,
        }
    })
    .to_string()
}

/// The agent's own session and every scope above it.  Other agents' lists
/// stay out so none picks up another's work.
fn visible_nodes(place: &Place, session: Option<&SessionRef>) -> Vec<String> {
    let mut nodes = vec![GLOBAL.to_string()];
    let repo = match place {
        Place::Global => None,
        Place::Project { repo } | Place::Workspace { repo, .. } => Some(repo.clone()),
    };
    nodes.extend(repo.map(|repo| node(&Place::Project { repo }, None)));
    if let Place::Workspace { .. } = place {
        nodes.push(node(place, None));
        if session.is_some() {
            nodes.push(node(place, session));
        }
    }
    nodes
}

pub(crate) fn context(place: &Place, session: Option<&SessionRef>, tasks: &[Task]) -> String {
    let target = node(place, session);
    let mut text = format!(
        "Task list, kept in taskwarrior. Write your own tasks to project `{target}`.\n\
         - add: `task add project:{target} order:<n> subof:<parent uuid> -- <text>` (subof is optional)\n\
         - finish: `task <uuid> done`; begin: `task <uuid> start`\n\
         `alacritree task scope` prints the project. If `subof:` or `order:` end up inside a \
         description, run `alacritree task setup` once.\n"
    );
    for scope in visible_nodes(place, session).iter().rev() {
        let in_scope: Vec<&Task> =
            tasks.iter().filter(|t| t.project.as_deref() == Some(scope.as_str())).collect();
        if in_scope.is_empty() {
            continue;
        }
        text.push_str(&format!("\n## {scope}\n"));
        for row in tree::rows(&in_scope) {
            let mark = if row.status == Status::Completed { "x" } else { " " };
            let started = if row.started { " (in progress)" } else { "" };
            let short = row.uuid.get(..8).unwrap_or(&row.uuid);
            let indent = "  ".repeat(row.depth);
            text.push_str(&format!("{indent}- [{mark}] {}{started} ({short})\n", row.text));
        }
    }
    text
}

fn digest_path(state_dir: &Path, session: &SessionRef) -> PathBuf {
    let name = format!("{}-{}.digest", session.harness.prefix(), sanitize(&session.id));
    state_dir.join("task-hooks").join(name)
}

/// `None` means print nothing, which is where every failure lands.
pub(crate) fn run(
    event: Event,
    harness: Harness,
    stdin: &str,
    state_dir: Option<&Path>,
) -> Option<String> {
    let payload: Payload = serde_json::from_str(stdin).unwrap_or_default();
    let cwd = payload.cwd.or_else(|| std::env::current_dir().ok())?;
    let session =
        payload.session_id.filter(|id| !id.trim().is_empty()).map(|id| SessionRef { harness, id });
    let (place, tasks) = jobs::on_this_thread(|b| {
        let (side, place) = facts::place_for(&cwd, b);
        let scopes = visible_nodes(&place, session.as_ref())
            .iter()
            .map(|n| format!("project.is:{n}"))
            .collect::<Vec<_>>()
            .join(" or ");
        let filter = [format!("({scopes})"), "(status:pending or status:completed)".to_string()];
        let tasks = Taskwarrior::for_side(side, b).export(&filter, b);
        tasks.map(|tasks| (place, tasks))
    })
    .ok()?;
    let text = context(&place, session.as_ref(), &tasks);
    // Session start records what it showed too, so the first prompt after it
    // does not repeat an unchanged list.
    if let (Some(dir), Some(session)) = (state_dir, session.as_ref()) {
        let path = digest_path(dir, session);
        let digest = format!("{:016x}", stable_digest(text.as_bytes()));
        let seen = std::fs::read_to_string(&path).is_ok_and(|s| s == digest);
        if seen && event == Event::UserPromptSubmit {
            return None;
        }
        if !seen {
            let _ = path.parent().map(std::fs::create_dir_all);
            let _ = std::fs::write(&path, digest);
        }
    }
    Some(output(event, &text))
}
```

In `cli/mod.rs`, add to `Command`:

```rust
    /// Run what a harness hook event needs.  Called from Claude Code's and
    /// codex's hook config; prints one JSON object or nothing.
    Hook {
        event: crate::tasks::hook::Event,
        /// Which harness is calling.  Hook processes do not inherit the
        /// variables a harness gives its own shell commands, so this cannot
        /// be read from the environment.
        #[arg(long, value_parser = ["claude", "codex"])]
        harness: String,
    },
```

and, next to `Command::Doctor`'s early return:

```rust
        // A harness waits on this before the model sees the turn, so it
        // answers from disk with no window and never fails the hook.
        Command::Hook { event, harness } => {
            let mut stdin = String::new();
            let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut stdin);
            let harness = crate::tasks::scope::Harness::parse(&harness)
                .expect("clap restricts --harness to known names");
            if let Some(out) = crate::tasks::hook::run(
                event,
                harness,
                &stdin,
                crate::state::config_dir().as_deref(),
            ) {
                println!("{out}");
            }
            return Some(0);
        },
```

Before `run`, load the config the way `Command::Doctor`'s path loads it from `cli.config_dir` and `cli.options`, then call `crate::tools::configure(config.integrations.tool_paths())`, so `[integrations.taskwarrior] path` and `-o` reach the adapter. A config that fails to load leaves the defaults in place, since the hook must still exit 0. `a_missing_task_binary_prints_nothing_and_exits_zero` catches a missing wiring on any machine with `task` installed. Add `| Command::Hook { .. }` to the `unreachable!` arm.

- [ ] **Step 6: Run to verify they pass**

Run: `devkit run task test --dir <worktree> -- hook`
Expected: 3 unit tests and 4 process tests pass. The two process tests that need `task` return early without it.

- [ ] **Step 7: Commit**

```sh
git -C <worktree> add alacritree/src alacritree/tests
git -C <worktree> commit -m "feat(cli): feed task lists to agent hooks" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 7: The tasks tab

**Files:**
- Create: `alacritree/src/tasks/view.rs`
- Modify: `alacritree/src/tasks/mod.rs` (`pub(crate) mod view;`)
- Modify: `alacritree/src/session.rs` (`SessionKind::Tasks`, `Session.tasks`, `spawn_tasks`, the PTY guards, `screen_snapshot`)
- Modify: `alacritree/src/bindings/action.rs` (`OpenTasks` after `OpenScratchpad`)
- Modify: `alacritree/src/bindings.rs` (variant, description, default binding, action lists, tests)
- Modify: `alacritree/src/app/actions.rs` (`impl Action for action::OpenTasks`)
- Modify: `alacritree/src/app.rs` (`toggle_tasks_tab`, the render branch at `app.rs:3191`, the `SessionKind` sites at `1437`, `1551`, `1671`)
- Modify: `alacritree/src/app/palette.rs`, `alacritree/src/app/ipc_handler.rs` (`SessionKind::Tasks` arms)
- Modify: `alacritree/src/command_palette.rs` (group at line 136, `action_items` gating)

**Interfaces:**
- Consumes: `tree::{sections, insert_after, indent, dedent, Edit, Section, Row}`, `Taskwarrior`, `TaskError`, `jobs::{pool, Priority, Job, Blocking}`, `projects::{Project, Worktree}`, `scope::{node, Place, GLOBAL}`, `multiplexer::Side`.
- Produces:
  - `view::Scope { side: Side, repo: Option<String>, workspace: Option<String> }`, `Scope::for_workspace(Option<&Project>, Option<&Worktree>) -> Scope`, `Scope::filter(&self) -> Vec<String>`
  - `view::TasksView::new(Scope) -> TasksView`, `TasksView::plain_lines(&self) -> Vec<String>`
  - `view::show(ui: &mut Ui, view: &mut TasksView, allow_focus: bool, text: Color32, dim: Color32, error: Color32) -> Response`
  - `action::OpenTasks`, `NamedAction::OpenTasks`

- [ ] **Step 1: Write the failing tests.** In `bindings.rs`, add `(Key::Backtick, ctrl_shift, OpenTasks(action::OpenTasks)),` to the table in `default_app_shortcuts_present_without_user_config`, and next to `scratchpad_tab_is_a_default_ctrl_backtick_binding_and_parses`:

```rust
#[test]
fn tasks_tab_is_a_default_ctrl_shift_backtick_binding_and_parses() {
    let b = parse_bindings(Vec::new());
    assert_eq!(
        named_matches(&b, Key::Backtick, Modifiers::CTRL | Modifiers::SHIFT),
        vec![NamedAction::OpenTasks(action::OpenTasks)]
    );
    assert_eq!(
        parse_action("OpenTasks"),
        BindingAction::Named(NamedAction::OpenTasks(action::OpenTasks))
    );
}
```

In `command_palette.rs`, next to `action_items_carry_keys_for_bound_actions` (build `shortcuts` the way that test does):

```rust
#[test]
fn the_tasks_action_is_listed_only_while_the_integration_is_on() {
    let has_tasks = |items: &[PaletteItem]| {
        items.iter().any(|i| matches!(i.action(), Some(NamedAction::OpenTasks(_))))
    };
    assert!(!has_tasks(&action_items(&shortcuts, false)));
    assert!(has_tasks(&action_items(&shortcuts, true)));
}
```

Use whatever accessor `PaletteItem` has for its action in place of `action()`.

In `tasks/view.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn project(root: &str, name: &str) -> Project {
        Project {
            root: PathBuf::from(root),
            name: name.into(),
            label: None,
            default_branch: None,
            worktrees: Vec::new(),
            expanded: false,
            shell_override: None,
            home: None,
        }
    }

    fn worktree(path: &str, name: &str, branch: Option<&str>) -> Worktree {
        Worktree {
            name: name.into(),
            path: PathBuf::from(path),
            branch: branch.map(Into::into),
            is_main: false,
            prunable: false,
            upstream: None,
        }
    }

    #[test]
    fn home_has_only_global() {
        let s = Scope::for_workspace(None, None);
        assert_eq!((s.repo, s.workspace), (None, None));
    }

    #[test]
    fn a_worktree_names_repo_and_branch() {
        let p = project("C:/src/alacritree", "alacritree");
        let w = worktree("C:/src/wt/feat", "feat", Some("feat/x"));
        let s = Scope::for_workspace(Some(&p), Some(&w));
        assert_eq!(s.repo.as_deref(), Some("alacritree"));
        assert_eq!(s.workspace.as_deref(), Some("alacritree.feat-x"));
    }

    #[test]
    fn a_detached_worktree_uses_its_directory_name() {
        let p = project("C:/src/r", "r");
        let w = worktree("C:/src/wt/review", "review", None);
        assert_eq!(Scope::for_workspace(Some(&p), Some(&w)).workspace.as_deref(), Some("r.review"));
    }

    #[test]
    fn a_non_git_root_has_a_project_section_only() {
        let s = Scope::for_workspace(Some(&project("C:/notes", "notes")), None);
        assert_eq!((s.repo.as_deref(), s.workspace), (Some("notes"), None));
    }

    #[cfg(windows)]
    #[test]
    fn a_distro_project_runs_on_its_distro() {
        let p = project(r"\\wsl.localhost\Ubuntu\home\lev\r", "r");
        assert_eq!(Scope::for_workspace(Some(&p), None).side, Side::Wsl("Ubuntu".into()));
    }

    #[test]
    fn the_filter_keeps_other_repos_out() {
        let s = Scope { side: Side::Native, repo: Some("r".into()), workspace: None };
        assert_eq!(s.filter(), [
            "(project.is:r or project:r. or project.is:global)".to_string(),
            "(status:pending or status:completed)".to_string(),
        ]);
    }

    #[test]
    fn home_filters_to_global() {
        let s = Scope::for_workspace(None, None);
        assert_eq!(s.filter()[0], "(project.is:global)");
    }
}
```

A non-git root passes `None` for the worktree: its pseudo-worktree has no branch and there is nothing to key a workspace on, which is what the spec asks for.

- [ ] **Step 2: Run to verify they fail**

Run: `devkit run task test --dir <worktree> -- tasks_tab tasks_action tasks::view`
Expected: FAIL to compile.

- [ ] **Step 3: Implement the action, binding and palette gating.**

`bindings/action.rs`: `OpenTasks,` after `OpenScratchpad,`.

`bindings.rs`: after `OpenScratchpad(action::OpenScratchpad),` in `NamedAction`:

```rust
    /// Open/select the current workspace's task list, or close it when active.
    OpenTasks(action::OpenTasks),
```

the description arm `Self::OpenTasks(_) => "Toggle the workspace tasks tab".into(),`, the default binding after the scratchpad's at line 839:

```rust
        KeyBinding {
            key: Key::Backtick,
            mods: Modifiers::CTRL | Modifiers::SHIFT,
            action: BindingAction::Named(OpenTasks(action::OpenTasks)),
        },
```

and `OpenTasks(action::OpenTasks),` in every list that enumerates each action (the one at line 2044 and any the compiler finds).

`command_palette.rs:136`: `OpenScratchpad(_) | OpenTasks(_) | TogglePalette(_) => Window,`. Give `action_items` a `tasks_enabled: bool` parameter and filter out `NamedAction::OpenTasks(_)` when it is false:

```rust
pub(crate) fn action_items(shortcuts: &Shortcuts, tasks_enabled: bool) -> Vec<PaletteItem> {
    let mut order: Vec<NamedAction> = NamedAction::iter()
        .filter(|a| !is_hidden(*a))
        .filter(|a| tasks_enabled || !matches!(a, NamedAction::OpenTasks(_)))
        .collect();
```

Pass `true` in the two existing tests and `app.config.integrations.taskwarrior.enabled` at the real call sites.

`app/actions.rs`:

```rust
impl Action for action::OpenTasks {
    fn run(&self, app: &mut AlacritreeApp, ctx: &Context, _: ActionOrigin) {
        if app.config.integrations.taskwarrior.enabled {
            app.toggle_tasks_tab(ctx);
        }
    }
}
```

- [ ] **Step 4: Implement the session kind.** In `session.rs`'s `SessionKind`:

```rust
    /// Task lists kept in taskwarrior, drawn as checklist rows.  No PTY, and
    /// nothing to save: taskwarrior holds every task.
    Tasks,
```

Add `pub tasks: Option<crate::tasks::view::TasksView>,` beside `pub scratchpad`, set it to `None` at the other construction sites (`session.rs:924`, `1551`), and add `spawn_tasks` beside `spawn_scratchpad` with the same body except: no `Editor::open`, `title: "tasks".to_string()`, `kind: SessionKind::Tasks`, `scratchpad: None`, `tasks: Some(view)`, and a `view: crate::tasks::view::TasksView` parameter in place of `path`. It returns `Self`, not `io::Result<Self>`, since nothing can fail. Every other field keeps the value `spawn_scratchpad` gives it.

The three `if self.scratchpad.is_some()` guards at `session.rs:1137`, `1150` and `1166` skip PTY work a document tab lacks; make each `if self.scratchpad.is_some() || self.tasks.is_some()`. In `screen_snapshot`, after the scratchpad branch:

```rust
        if let Some(view) = &self.tasks {
            let lines = view.plain_lines();
            let cursor_line = lines.len().saturating_sub(1);
            return ScreenSnapshot { lines, cursor_line, cursor_column: 0, history_size: 0 };
        }
```

Fix every non-exhaustive `SessionKind` match the compiler reports: `ipc_handler.rs:45` and `palette.rs:994` get `SessionKind::Tasks => "tasks"`, `palette.rs:1004` joins the `None` arm, and `app.rs:1551` joins the `Scratchpad | Diff` arm.

- [ ] **Step 5: Implement `tasks/view.rs`** above its tests:

```rust
//! The tasks tab: taskwarrior's lists for one workspace, drawn as checklist
//! rows.  Each change is one `task` call on the pool, typed text shows at
//! once, and a reload replaces it with what taskwarrior holds.  Agents write
//! the same store, so the tab re-exports while visible instead of trusting
//! its own copy.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use egui::{Color32, Key, Modifiers, Response, RichText, ScrollArea, Sense, TextEdit, Ui};

use crate::jobs::{self, Blocking, Job, Priority};
use crate::multiplexer::Side;
use crate::projects::{Project, Worktree};
use crate::tasks::scope::{GLOBAL, Place, node};
use crate::tasks::taskwarrior::{Status, Task, TaskError, Taskwarrior};
use crate::tasks::tree::{self, Edit, Row, Section};
use crate::wsl;

const RELOAD_EVERY: Duration = Duration::from_secs(1);
const INDENT: f32 = 16.0;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Scope {
    pub side: Side,
    pub repo: Option<String>,
    pub workspace: Option<String>,
}

impl Scope {
    /// Reads the sidebar's model, never git: a WSL worktree is a UNC path
    /// Windows-side git reads wrongly, and the sidebar already has both names.
    pub(crate) fn for_workspace(project: Option<&Project>, worktree: Option<&Worktree>) -> Self {
        let side = match project.map(|p| wsl::classify(&p.root)) {
            Some(wsl::Location::Wsl { distro, .. }) => Side::Wsl(distro),
            _ => Side::Native,
        };
        let workspace = project.zip(worktree).map(|(p, wt)| {
            let branch = wt.branch.clone().unwrap_or_else(|| wt.name.clone());
            node(&Place::Workspace { repo: p.name.clone(), branch }, None)
        });
        let repo = project.map(|p| node(&Place::Project { repo: p.name.clone() }, None));
        Self { side, repo, workspace }
    }

    /// `project:` is a left match, so `project:r` alone would also return a
    /// repository named `r-web`; the trailing dot keeps to `r`'s subtree.
    pub(crate) fn filter(&self) -> Vec<String> {
        let scopes = match &self.repo {
            Some(repo) => format!("(project.is:{repo} or project:{repo}. or project.is:{GLOBAL})"),
            None => format!("(project.is:{GLOBAL})"),
        };
        vec![scopes, "(status:pending or status:completed)".to_string()]
    }
}

/// A row being typed that taskwarrior has not got yet.
struct NewRow {
    node: String,
    after: Option<String>,
    depth: usize,
    text: String,
    focus: bool,
}

pub(crate) struct TasksView {
    scope: Scope,
    tasks: Vec<Task>,
    load_error: Option<String>,
    reload: Option<Job<Result<Vec<Task>, TaskError>>>,
    last_reload: Option<Instant>,
    /// In-flight writes, each with the row its failure is shown on.
    writes: Vec<(Option<String>, Job<Result<(), TaskError>>)>,
    row_errors: HashMap<String, String>,
    /// Typed text a reload has not confirmed yet, by uuid.
    drafts: HashMap<String, String>,
    new_row: Option<NewRow>,
    collapsed: HashSet<String>,
}

impl TasksView {
    pub(crate) fn new(scope: Scope) -> Self {
        Self {
            scope,
            tasks: Vec::new(),
            load_error: None,
            reload: None,
            last_reload: None,
            writes: Vec::new(),
            row_errors: HashMap::new(),
            drafts: HashMap::new(),
            new_row: None,
            collapsed: HashSet::new(),
        }
    }

    fn sections(&self) -> Vec<Section> {
        tree::sections(&self.tasks, self.scope.repo.as_deref(), self.scope.workspace.as_deref())
    }

    pub(crate) fn plain_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for section in self.sections() {
            lines.push(format!("## {}", section.node));
            for row in section.rows {
                let mark = if row.status == Status::Completed { "x" } else { " " };
                lines.push(format!("{}- [{mark}] {}", "  ".repeat(row.depth), row.text));
            }
        }
        lines
    }

    fn section_tasks(&self, node: &str) -> Vec<Task> {
        self.tasks.iter().filter(|t| t.project.as_deref().unwrap_or(GLOBAL) == node).cloned().collect()
    }

    fn write(
        &mut self,
        row: Option<String>,
        work: impl FnOnce(&Taskwarrior, &Blocking) -> Result<(), TaskError> + Send + 'static,
    ) {
        if let Some(uuid) = &row {
            self.row_errors.remove(uuid);
        }
        let side = self.scope.side.clone();
        let job = jobs::pool()
            .spawn(Priority::Interactive, move |b| work(&Taskwarrior::for_side(side, b), b));
        self.writes.push((row, job));
    }

    fn apply(&mut self, row: Option<String>, edits: Vec<Edit>) {
        if edits.is_empty() {
            return;
        }
        self.write(row, move |tw, b| {
            for edit in edits {
                match edit {
                    Edit::Add { project, description, subof, order } => {
                        tw.add(&project, &description, subof.as_deref(), order, b)?;
                    },
                    Edit::Modify { uuid, mods } => tw.modify(&uuid, &mods, b)?,
                }
            }
            Ok(())
        });
    }

    /// Drains finished jobs, and reloads after any write or once a second.
    fn tick(&mut self) {
        let mut finished = Vec::new();
        self.writes.retain(|(row, job)| match job.poll() {
            Some(result) => {
                finished.push((row.clone(), result));
                false
            },
            None => !job.failed(),
        });
        let wrote = !finished.is_empty();
        for (row, result) in finished {
            let Err(e) = result else { continue };
            match row {
                Some(uuid) => {
                    self.drafts.remove(&uuid);
                    self.row_errors.insert(uuid, e.to_string());
                },
                None => self.load_error = Some(e.to_string()),
            }
        }
        if let Some(result) = self.reload.as_ref().and_then(Job::poll) {
            self.reload = None;
            match result {
                Ok(tasks) => {
                    self.load_error = None;
                    // A draft stays until the store holds its text or the
                    // task is gone.
                    self.drafts
                        .retain(|uuid, text| tasks.iter().any(|t| &t.uuid == uuid && &t.description != text));
                    self.tasks = tasks;
                },
                Err(e) => self.load_error = Some(e.to_string()),
            }
        }
        let due = self.last_reload.is_none_or(|t| t.elapsed() >= RELOAD_EVERY);
        if self.reload.is_none() && (wrote || due) {
            let scope = self.scope.clone();
            self.reload = Some(jobs::pool().spawn(Priority::Background, move |b| {
                Taskwarrior::for_side(scope.side.clone(), b).export(&scope.filter(), b)
            }));
            self.last_reload = Some(Instant::now());
        }
    }
}

pub(crate) fn show(
    ui: &mut Ui,
    view: &mut TasksView,
    allow_focus: bool,
    text: Color32,
    dim: Color32,
    error: Color32,
) -> Response {
    view.tick();
    ui.ctx().request_repaint_after(RELOAD_EVERY);
    let colors = Colors { text, dim, error };
    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        if let Some(e) = &view.load_error {
            ui.label(RichText::new(e).color(error));
        }
        for section in view.sections() {
            show_section(ui, view, &section, allow_focus, colors);
        }
    });
    ui.interact(ui.min_rect(), ui.id().with("tasks-tab"), Sense::click())
}

#[derive(Clone, Copy)]
struct Colors {
    text: Color32,
    dim: Color32,
    error: Color32,
}

fn show_section(ui: &mut Ui, view: &mut TasksView, section: &Section, allow_focus: bool, c: Colors) {
    let open = !view.collapsed.contains(&section.node);
    let done = section.rows.iter().filter(|r| r.status == Status::Completed).count();
    let arrow = if open { "v" } else { ">" };
    let header = format!("{arrow} {}  {done}/{}", section.node, section.rows.len());
    if ui.selectable_label(false, RichText::new(header).color(c.text).strong()).clicked() {
        if open {
            view.collapsed.insert(section.node.clone());
        } else {
            view.collapsed.remove(&section.node);
        }
    }
    if !open {
        return;
    }
    let tasks = view.section_tasks(&section.node);
    for row in &section.rows {
        show_row(ui, view, section, &tasks, row, allow_focus, c);
        let follows = |n: &NewRow| n.node == section.node && n.after.as_deref() == Some(&row.uuid);
        if view.new_row.as_ref().is_some_and(follows) {
            show_new_row(ui, view, &tasks);
        }
    }
    let adding_first = |n: &NewRow| n.node == section.node && n.after.is_none();
    if view.new_row.as_ref().is_some_and(adding_first) {
        show_new_row(ui, view, &tasks);
    } else if ui.small_button(RichText::new("+ add a task").color(c.dim)).clicked() {
        let last_root = section.rows.iter().rev().find(|r| r.depth == 0).map(|r| r.uuid.clone());
        view.new_row = Some(NewRow {
            node: section.node.clone(),
            after: last_root.clone(),
            depth: 0,
            text: String::new(),
            focus: true,
        });
        if last_root.is_some() {
            // Drawn after that row on the next frame, not here.
            return;
        }
    }
}

fn show_row(
    ui: &mut Ui,
    view: &mut TasksView,
    section: &Section,
    tasks: &[Task],
    row: &Row,
    allow_focus: bool,
    c: Colors,
) {
    let refs: Vec<&Task> = tasks.iter().collect();
    ui.horizontal(|ui| {
        ui.add_space(row.depth as f32 * INDENT);
        let mut checked = row.status == Status::Completed;
        if ui.checkbox(&mut checked, "").changed() {
            let uuid = row.uuid.clone();
            view.write(Some(row.uuid.clone()), move |tw, b| {
                if checked { tw.done(&uuid, b) } else { tw.undone(&uuid, b) }
            });
        }
        if row.started {
            ui.label(RichText::new(">").color(c.text));
        }
        let mut buffer = view.drafts.get(&row.uuid).cloned().unwrap_or_else(|| row.text.clone());
        // Locking focus keeps Tab for indenting instead of moving to the
        // next widget.
        let edit = ui.add_enabled(
            allow_focus,
            TextEdit::singleline(&mut buffer)
                .frame(false)
                .lock_focus(true)
                .text_color(if checked { c.dim } else { c.text })
                .desired_width(f32::INFINITY),
        );
        if edit.changed() {
            view.drafts.insert(row.uuid.clone(), buffer.clone());
        }
        edit.context_menu(|ui| {
            for (label, start) in [("Start", true), ("Stop", false)] {
                if ui.button(label).clicked() {
                    let uuid = row.uuid.clone();
                    view.write(Some(row.uuid.clone()), move |tw, b| {
                        if start { tw.start(&uuid, b) } else { tw.stop(&uuid, b) }
                    });
                    ui.close_menu();
                }
            }
        });
        if edit.has_focus() {
            let (tab, shift_tab, erase) = ui.input_mut(|i| {
                (
                    i.consume_key(Modifiers::NONE, Key::Tab),
                    i.consume_key(Modifiers::SHIFT, Key::Tab),
                    buffer.is_empty() && i.consume_key(Modifiers::NONE, Key::Backspace),
                )
            });
            if tab {
                view.apply(Some(row.uuid.clone()), tree::indent(&refs, &row.uuid));
            }
            if shift_tab {
                view.apply(Some(row.uuid.clone()), tree::dedent(&refs, &row.uuid));
            }
            if erase {
                view.drafts.remove(&row.uuid);
                let uuid = row.uuid.clone();
                view.write(Some(row.uuid.clone()), move |tw, b| tw.delete(&uuid, b));
            }
        }
        if edit.lost_focus() {
            let text = buffer.trim().to_string();
            if text != row.text && !text.is_empty() {
                let uuid = row.uuid.clone();
                view.write(Some(row.uuid.clone()), move |tw, b| tw.describe(&uuid, &text, b));
            }
            if ui.input(|i| i.key_pressed(Key::Enter)) {
                view.new_row = Some(NewRow {
                    node: section.node.clone(),
                    after: Some(row.uuid.clone()),
                    depth: row.depth,
                    text: String::new(),
                    focus: true,
                });
            }
        }
        if let Some(e) = view.row_errors.get(&row.uuid) {
            ui.label(RichText::new(e).color(c.error));
        }
    });
}

/// Taskwarrior rejects an empty description, so a new row exists only here
/// until it has text; leaving it empty drops it.
fn show_new_row(ui: &mut Ui, view: &mut TasksView, tasks: &[Task]) {
    let Some(new) = view.new_row.as_mut() else { return };
    let mut committed = None;
    let mut dropped = false;
    ui.horizontal(|ui| {
        ui.add_space(new.depth as f32 * INDENT);
        let edit = ui.add(TextEdit::singleline(&mut new.text).hint_text("new task").desired_width(f32::INFINITY));
        if new.focus {
            edit.request_focus();
            new.focus = false;
        }
        if edit.lost_focus() {
            let text = new.text.trim().to_string();
            if text.is_empty() { dropped = true } else { committed = Some(text) }
        }
    });
    if dropped {
        view.new_row = None;
    }
    if let Some(text) = committed {
        let new = view.new_row.take().expect("present above");
        let refs: Vec<&Task> = tasks.iter().collect();
        let edits = tree::insert_after(&refs, &new.node, new.after.as_deref(), &text);
        view.apply(None, edits);
    }
}
```

The borrow checker may object to `view.write` inside the `context_menu` closure while `edit` is alive, or to `show_section` iterating `view.sections()` while passing `view` mutably. Both are fixed by cloning the small values the closure needs first; `sections()` already returns an owned `Vec`. Keep the behaviour exactly as written: which keys do what, when a reload runs, and where errors show. Check `lock_focus`, `add_enabled`, `consume_key`, `context_menu`, `close_menu` and `is_none_or` against the egui version in `Cargo.toml` and the MSRV of 1.85, and adjust names if the API differs.

- [ ] **Step 6: Wire the tab into `app.rs`.** Next to `toggle_scratchpad_tab` (`app.rs:1058`), add a `tasks_session_index(&self, workspace: &WorkspaceKey) -> Option<usize>` copied from `scratchpad_session_index`, matching `SessionKind::Tasks`, and:

```rust
    fn toggle_tasks_tab(&mut self, ctx: &Context) {
        let workspace = self.current_workspace.clone();
        if let Some(index) = self.tasks_session_index(&workspace) {
            let id = self.sessions[index].id;
            if self.sessions.active(&workspace) == Some(id) {
                // Taskwarrior holds every task, so there is nothing to lose.
                self.close_session(ctx, id);
                return;
            }
            self.sessions.set_active(workspace, id);
        } else {
            let (project, worktree) = self.project_and_worktree(&workspace);
            let scope = crate::tasks::view::Scope::for_workspace(project, worktree);
            let session = Session::spawn_tasks(
                ctx.clone(),
                &self.config,
                workspace.clone(),
                TermSize::new(80, 24),
                (8.0, 16.0),
                crate::tasks::view::TasksView::new(scope),
            );
            let id = session.id;
            self.sessions.push(session);
            self.sessions.set_active(workspace, id);
        }
        self.focus_terminal();
    }
```

`project_and_worktree(&WorkspaceKey) -> (Option<&Project>, Option<&Worktree>)` returns `(None, None)` for home and otherwise the project whose `worktrees` holds the key's path, with that worktree. A non-git project root returns `(Some(project), None)` because its pseudo-worktree has no branch. If `app.rs` already has a lookup from a workspace key to its project and worktree (the git panel and the sidebar both need one), call it rather than adding another.

In the render branch at `app.rs:3191`, between the scratchpad arm and the terminal arm:

```rust
                } else if let Some(view) = session.tasks.as_mut() {
                    self.ime.clear();
                    crate::tasks::view::show(ui, view, allow_focus, editor_text, editor_hint, editor_error)
```

At `app.rs:1437` and `1671`, where a scratchpad tab skips the close prompt, give `SessionKind::Tasks` the same treatment.

- [ ] **Step 7: Run everything**

Run: `devkit run task fmt --dir <worktree>`, `devkit run task clippy --dir <worktree>`, `devkit run task test --dir <worktree>`
Expected: clean clippy and every test passing, including `tests/steady_state.rs`.

- [ ] **Step 8: Try it by hand.** With `task` installed and `alacritree task setup` run, start a window against a scratch config dir whose `alacritree.toml` has `[integrations.taskwarrior] enabled = true`:

```sh
cargo run -p alacritree -- --config-dir <scratch>
```

In a worktree, press Ctrl+~ and check:
- The global, project and workspace sections appear.
- "+ add a task", typing, then Enter creates a task. `task project:<repo>. export` shows it with `order:1024`.
- Tab on the second row nests it under the first, and Shift+Tab un-nests it.
- The checkbox completes a task, and clearing it makes the task pending again.
- `task add project:<workspace node>.codex-x -- hi`, run in another shell, adds a session section within about a second.
- With `enabled = false`, Ctrl+~ does nothing and the palette lists no tasks action.

- [ ] **Step 9: Commit**

```sh
git -C <worktree> add alacritree/src
git -C <worktree> commit -m "feat(tasks): add the tasks tab" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 8: Docs

**Files:**
- Modify: `docs/alacritree.md`
- Modify: `AGENTS.md`

- [ ] **Step 1: Document the table** in `docs/alacritree.md`, next to `[integrations.herdr]`, in the file's commented-default style:

```toml
[integrations.taskwarrior]
# enabled = false   # the tasks tab (OpenTasks, Ctrl+~) and its palette entry
# path = "task"     # the task binary on Windows or natively
# wsl_path = ""     # inside every distro; empty finds it by name
```

- [ ] **Step 2: Add a "Tasks" section.** Cover:
  - the four scopes and their node names
  - running `alacritree task setup` once per machine, and `alacritree task scope`
  - what the tab shows: global, project, workspace, and each agent session under the workspace
  - the row keys: Enter, Tab, Shift+Tab, Backspace on an empty row, and right-click for start and stop
  - the hook config for each harness

Claude Code `settings.json`:

```json
{
  "hooks": {
    "SessionStart": [{ "hooks": [{ "type": "command", "command": "alacritree hook session-start --harness claude" }] }],
    "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "alacritree hook user-prompt-submit --harness claude" }] }]
  }
}
```

codex `hooks.json`:

```json
{
  "SessionStart": [{ "hooks": [{ "type": "command", "command": "alacritree hook session-start --harness codex" }] }],
  "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "alacritree hook user-prompt-submit --harness codex" }] }]
}
```

The codex shape follows codex's own test at `codex-rs/hooks/src/engine/mod_tests.rs:1671`, which writes events at the top level. Confirm it against `codex-rs/hooks/src/engine/discovery.rs` (`docm info codex`) before committing, and fix the example if the file nests events under a key.

Soft-wrap the prose, one line per paragraph, and write no counts that go stale.

- [ ] **Step 3: Add to `AGENTS.md`'s architecture list**, after the `scratchpad.rs` bullet:

```markdown
- `tasks/` holds task lists kept in taskwarrior. `scope.rs` names project nodes, `facts.rs` reads git on the cwd's side, `taskwarrior.rs` is the only code that runs `task`, `tree.rs` is the tab's pure model, `hook.rs` backs `alacritree hook`, and `view.rs` draws the tab. Off unless `[integrations.taskwarrior] enabled`.
```

- [ ] **Step 4: Commit**

```sh
git -C <worktree> add docs/alacritree.md AGENTS.md
git -C <worktree> commit -m "docs(tasks): document the taskwarrior integration" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## After the last task

- Settle the spec's "To verify during planning" items and note the results in the PR body:
  - In a codex session with `shell_environment_policy.include_only` set, run `echo $CODEX_SESSION_ID`. If it is empty, make `session_from_env` fall back to `CODEX_THREAD_ID`, and say in the PR that codex subagents then get nodes of their own.
  - `add_returns_the_uuid_and_round_trips_subof_and_order` passing shows `export` emits `subof` and `order` when they are declared only by overrides.
- Before opening a PR, cherry-pick the branch into the `all-features` worktree and run `install.local.py` there, per `AGENTS.local.md`.
