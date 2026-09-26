# Task tracking through taskwarrior

Issue: AbysmalBiscuit/alacritree#86

## Goal

Agents and humans share hierarchical task lists that live outside alacritree. Any harness with a shell can use them, including codex, and they keep working when only herdr is running. alacritree adds a view for reading and editing them, and a hook command that keeps agents in sync with human edits.

This is v1. Taskwarrior is the store. Whether alacritree later grows a native store is a separate decision, to be made with Arnaud.

## Decisions

- Taskwarrior 3 is the store and `task` is the interface agents write through. alacritree never writes tasks anywhere else.
- Scopes nest four levels deep: global, project, workspace, agent session. Each scope is a taskwarrior project node.
- Tasks inside a scope form a tree through a `subof` UDA of type `uuid` holding the parent task's uuid. Siblings sort by an integer `order` UDA.
- `parent` is not usable. It is a core taskwarrior attribute linking recurring instances to their template: declaring a UDA with that name aborts every command, and setting it marks a task as a recurrence instance.
- The session scope is the agent's conversation, keyed by the harness's own id, so `claude --resume` and `codex resume` get their tasks back.
- v1 ships no MCP tools. Every target harness has a shell.
- The feature is off by default under `[integrations.taskwarrior] enabled`, so a stock config behaves as it does today.

## Scope naming

One pure function turns repository facts and a session id into a project name:

| Scope | Project node | Example |
|---|---|---|
| global | `global` | `global` |
| project | `<repo>` | `alacritree` |
| workspace | `<repo>.<branch>` | `alacritree.feat-86-task-tracking` |
| agent session | `<repo>.<branch>.<harness>-<id>` | `alacritree.feat-86-task-tracking.codex-0199a...` |

- `<repo>` is the basename of the main worktree's top-level directory, so every worktree of one repo shares a project node. For a bare repository it is the common dir's basename with a trailing `.git` removed. For a submodule it is the submodule's own top-level basename, not the `modules` directory its common dir lives in.
- `<branch>` is the worktree's checked-out branch. Git refuses to check one branch out in two worktrees unless forced. A detached HEAD, or a branch forced into a second worktree, falls back to the worktree directory name.
- `.` and `/` in either name become `-`, since taskwarrior uses `.` as its hierarchy separator.
- `<harness>-<id>` is `claude-<id>` or `codex-<id>`. Where the id comes from depends on the caller, see below. With no id, the scope stops at the workspace.
- A directory outside any git repository has scope `global`.

Two repos whose main checkouts share a directory name share a project node. That is accepted in exchange for names a human can read in `task projects`.

### Where each caller gets its facts

| Caller | Repo and branch | Session id |
|---|---|---|
| `alacritree task scope`, run in an agent's shell | `git` on the cwd's side | `CODEX_SESSION_ID`, else `CLAUDE_CODE_SESSION_ID` |
| `alacritree hook <event>` | `git` on the payload `cwd`'s side | the payload's `session_id`, harness from `--harness` |
| The view | the sidebar model's project and worktree | none, the view lists every session under the workspace |

- Codex injects `CODEX_THREAD_ID` and `CODEX_SESSION_ID` into shell commands the model runs, but not into hook processes, which get the codex process's own environment. The hook payload's `session_id` is the root session id, which a subagent shares while its thread id differs. Keying codex on `CODEX_SESSION_ID` keeps the shell side and the hook side on the same node.
- `CLAUDE_CODE_SESSION_ID` is set in Claude Code's Bash tool but is undocumented. Claude session ids survive `--resume` unless `--fork-session` is used. The hook's stdin `session_id` is the authoritative source, and the env var is relied on only for the shell side.
- The CLI and the hook pick the side from the cwd with `wsl::classify`: a `\\wsl.localhost\<distro>\...` cwd means that distro, which is what a Windows binary launched from inside WSL sees. Repository facts come from the `git` CLI run on that side through `Side::command`, never from git2, so a distro path is read by the distro's own git. `task` runs on the same side.
- The view never runs git on a cwd. A WSL worktree is a `\\wsl.localhost\...` path where Windows-side git is the wrong tool, and the sidebar already knows each worktree's project and branch.
- The home workspace has no repo, so its tab shows only the global section.
- A non-git project root, which the sidebar lists through a pseudo-worktree, uses the project root's directory name as `<repo>` and has no workspace or session sections.

## Components

- `tasks/scope.rs` holds the scope function. It takes already-resolved facts (repo name, branch or fallback, harness and id) so the three callers share it without sharing how they gather facts.
- `tasks/taskwarrior.rs` is the only module that runs `task`. It builds children with `command_ext::hidden` and runs them on the `jobs` pool, `Priority::Interactive` for edits and `Priority::Background` for reloads. A WSL workspace goes through `Side::command`, so it uses that distro's taskwarrior.
  - Reads use `export` into `Task { uuid, description, status, start, subof, order, project, modified }`.
  - Writes use `add`, `modify`, `done`, `start`, `stop` and `delete`, always addressed by uuid.
  - Every call passes the UDA declarations as `rc.uda.*` overrides and `rc.confirmation=off`. `add` also passes `rc.verbose=new-uuid` and reads the new uuid from its output.
  - taskchampion opens each write as an immediate SQLite transaction with no busy timeout, so a concurrent writer fails at once. The adapter retries a busy write a few times with a short backoff before reporting it.
- The CLI gains `alacritree task scope`, which prints the scope for the cwd, `alacritree task setup`, which declares the UDAs, and `alacritree hook <event> --harness <claude|codex>`. All three are local commands like `doctor`: no IPC and no running window.
- `alacritree hook <event>` is one entry point for every harness hook. v1 handles `session-start` and `user-prompt-submit`. Later features add work under the same events without users editing their harness config again.
- The view is a `SessionKind::Tasks` tab opened by a new `OpenTasks` action, bound by default to Ctrl+Shift and the physical backtick key, spelled `Grave` or `Backtick` in config. That is Ctrl+~ on US layouts and sits next to the scratchpad's Ctrl+Backtick. egui has no `~` key, so the binding reaches `Key::Backtick` through egui-winit's physical-key fallback.
- `[integrations.taskwarrior]` holds `enabled = false`, plus `path` and `wsl_path` for the `task` binary through the existing `tool_config` pattern and a new `Tool::Task` variant, as `gh` does. Bindings are static, so with the feature off the default binding stays and `OpenTasks` does nothing, and the palette leaves the action out. Which database is used follows the user's `TASKRC` and `TASKDATA` on each side, so keeping agent tasks apart from a personal list is taskwarrior configuration.

## UDAs

Taskwarrior only parses `subof:` and `order:` once they are declared. Undeclared, `task add fix it order:3` stores `order:3` inside the description.

- `alacritree task setup` writes `uda.subof.type=uuid` and `uda.order.type=numeric`, with labels, through `task rc.confirmation=off config <key> <value>`, so taskwarrior itself finds and edits the taskrc. A key whose `task _get rc.<key>` already holds the value is skipped. It runs per side: the Windows taskrc and each distro's.
- `alacritree doctor` reports missing declarations per side, alongside the distro probes it already runs.
- Every reader and writer, meaning the view, the hook and the adapter, passes the same declarations as `rc.uda.*` overrides, so everything works before setup runs. Setup exists for agents calling `task` directly.

### Sibling order

Numeric UDAs are stored as text with 6 significant digits, so repeated halving between two neighbours collides after about 16 inserts. `order` is an integer with a stride of 1024 between siblings. A new sibling takes the midpoint of the gap it lands in. When no gap remains, the view renumbers that sibling set at the stride before inserting. The list reloads after every write, so a renumber is invisible.

## The view

- One export, `(project.is:<repo> or project:<repo>. or project.is:global) (status:pending or status:completed)`. The trailing `.` matters: `project:` is a left match, so a bare `project:alacritree` also returns `alacritree-web`. Filtering by status keeps waiting and recurring tasks from a personal list out.
- The result is split into sections by project node: global, project, workspace, then one section per agent session under the workspace, most recently modified first. Every section collapses.
- Each row is a checkbox and a one-line text field. Rows nest by `subof` and sort by `order`.
- Unchecked is `pending`, checked is `completed` and drawn dimmed, and in progress is `pending` with `start` set, drawn with a marker.

| Input | Effect |
|---|---|
| Click the checkbox | `done`, or `modify status:pending` to uncheck |
| Enter | `add` a sibling below, with `order` in the gap after the current row |
| Tab | `modify subof:` to the previous sibling, `order` after that sibling's last child |
| Shift+Tab | `modify subof:` to the current parent's parent, `order` right after the old parent |
| Backspace on an empty row | `delete` |
| Right-click | Menu with start and stop |

- Text commits on Enter or when the row loses focus, never per keystroke.
- Each edit is one job on the `jobs` pool. The row shows the change immediately and the list reloads when the job returns.
- While the tab is visible, the view re-exports on a background job about once a second. taskchampion runs SQLite in WAL mode, so the main database file's mtime does not move on commit, and for a WSL workspace the database lives inside the distro. Re-exporting avoids both problems. A reload whose result equals the current list changes nothing on screen.

## Hooks

- Each harness's hook config calls `alacritree hook <event> --harness claude` or `--harness codex`. The payload arrives on stdin, and the hook takes `session_id` and `cwd` from it.
- `session-start` injects the agent's own session list, the workspace list, and the project and global lists, followed by a short reminder to write with `project:$(alacritree task scope)` and the `subof` and `order` attributes. Other agents' session lists are left out.
- `user-prompt-submit` injects the same payload only when its digest differs from the one that agent session last saw. Digests live under the state dir, keyed by harness and session id. This is how a human's edit in the view reaches the agent.
- Output is one JSON object on stdout and nothing else, for both harnesses: `{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"..."}}`, with `UserPromptSubmit` as the event name for the second hook. Codex parses stdout only as JSON and rejects unknown fields. When there is nothing to inject, the hook prints nothing.
- Codex hooks are enabled by default and load from `hooks.json`.

## Failure handling

No failure here affects a terminal session.

- `task` missing on a side: the tab says taskwarrior was not found on that side and names the resolved path. The hook exits 0 and prints nothing, so a missing tool never blocks an agent's turn.
- UDAs undeclared: the tab works through its overrides and shows a one-line banner pointing at `alacritree task setup`. The hook's reminder says the same.
- A write fails after its busy retries, or on an unknown uuid, or on a task an agent deleted mid-edit: the row reverts, shows the error in place the way the scratchpad shows `save_error`, and the next reload shows the real state.
- The hook's `cwd` is outside a repo: the scope is `global` and the hook injects only the global list.
- The hook cannot parse stdin or the payload has no `session_id`: it injects the workspace, project and global lists and skips the session part.

## Testing

- `scope`: table tests over resolved facts, covering sanitizing, detached HEAD, a forced duplicate branch, a bare repo, a submodule, a non-git root, codex, claude, no id, and outside a repo.
- The adapter: integration tests against a real `task` with a temporary `TASKDATA` and a temporary `TASKRC`, so the user's rc and its hooks never run. Skipped when `task` is not on PATH. They cover export parsing, `subof` and `order` round-trips, uuid capture from `add`, `delete` without a prompt, every write verb by uuid, and a busy retry.
- The hook: runs the built binary through `command_ext::hidden` with `env!("CARGO_BIN_EXE_alacritree")`, as `tests/cli_isolation.rs` does, fed recorded Claude and codex payloads for both events. It asserts the exact JSON on stdout, that a second prompt with an unchanged list prints nothing, and that a missing `task` prints nothing and exits 0.
- The view model: a pure tree built from `Vec<Task>`, free of egui like `sidebar_nav.rs`. It tests the section split, sibling order, what indent and dedent do to `subof` and `order`, and renumbering when a gap runs out.
- Config: schema defaults for `[integrations.taskwarrior]`, `OpenTasks` doing nothing when disabled, and no palette entry when disabled.

## Out of scope for v1

- MCP tools.
- Mirroring native todo tools (`TodoWrite`, `update_plan`) into taskwarrior.
- Showing the focused tab's agent first, which needs a mapping from tab to conversation id.
- Moving tasks when a branch is renamed. They stay under the old node.
- Claiming tasks and conflict rules for a shared work queue.
- A native store.

## To verify during planning

- `CODEX_SESSION_ID` reaches model-run shells under `include_only` the way `CODEX_THREAD_ID` does.
- `export` emits undeclared UDA values when the declarations come only from `rc.uda.*` overrides.
