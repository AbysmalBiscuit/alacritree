# Plan for issue 94: break the cycles below the god module

Spec: `C:\Users\Lev\Git\github\alacritree-worktrees\ISSUE_SUMMARY_94.md` (GitHub issue https://github.com/AbysmalBiscuit/alacritree/issues/94)

Four items live in `alacritree/src/app.rs` and `alacritree/src/main.rs` that nothing about them requires. They are the six `crate::app::` back-edges that make the module graph cyclic below `app`. Each task removes one group of edges. After Task 4, `rg -n 'crate::app::' alacritree/src --glob '*.rs' | rg -v 'src.app\.rs'` returns nothing.

Module placement was decided with the project's architecture session (the `primary` agent) before this plan was written. Do not relitigate the target module names.

## Global Constraints

- **No behavior change.** Every task is a relocation. Moved code keeps its body byte-for-byte where the move allows. Only these edits are permitted on moved code: `use` paths, item visibility, and the `crate::`-relative paths inside a moved body that the new location breaks.
- **Keep every doc comment verbatim** with the item it documents. Do not rewrite, shorten, or improve them. Where a doc comment names a symbol by a path that the move changes (for example `NOTIFY_TX`, `notify_macos::init`), update only that path reference.
- **Do not add comments** narrating the move. No "moved from app.rs", no issue references. The project's comment rules are in `AGENTS.md`: explain a non-obvious why, never restate the what.
- **Moved tests move with their subject.** A test that exercises a moved item goes into the new module's own `#[cfg(test)] mod tests`, along with any test helper used only by those tests. Do not write new tests. These are relocations, and a new test for a moved function proves nothing the existing one does not.
- **Register every new module** in `alacritree/src/main.rs`'s `mod` list, alphabetically, matching the existing `#[cfg(...)]` style of its neighbours.
- **Claim files before editing.** This checkout is shared with other agents and `[harness] enforce_writes` refuses unclaimed writes:
  `DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm acquire <absolute path> --note "issue 94 task N"`
- **Absolute paths, never `cd`.** Use `git -C <abs repo>` and write out full paths in every command.
- **Verification per task, in this order:** `devrun task fmt`, `devrun task check`, `devrun task test`, `devrun task clippy`. All four must pass before the task is reported DONE. Put the actual command output in the report file.
- **One commit per task.** Conventional Commits, imperative subject, at most 72 chars, body wrapped at 72. End every commit message with:
  `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`
- Do not run `git stash`. Do not push. Do not open a PR.
- `cargo test` is not the command here. `devrun task test` runs nextest, which is what this checkout uses.

## Task 1: move `WorkspaceKey` to a new `workspace` module

Create `alacritree/src/workspace.rs` holding exactly the alias and its doc comment, currently `alacritree/src/app.rs:53`:

```rust
/// `None` is the home workspace (sessions inherit `$PWD`); `Some` is a worktree path.
pub type WorkspaceKey = Option<PathBuf>;
```

The new module needs `use std::path::PathBuf;`. Give the file a `//!` module header in the style of `path_style.rs` or `row_label.rs`: one or two sentences on what a workspace key is, no change narration.

Then:

1. Delete the alias from `app.rs` and add `use crate::workspace::WorkspaceKey;` there. `app.rs` has 89 references to the name, so the import must land in the existing `use crate::...` block and all 89 uses must still resolve unqualified.
2. Repoint the four importers from `use crate::app::WorkspaceKey;` to `use crate::workspace::WorkspaceKey;`: `sidebar_nav.rs:10`, `sidebar_focus.rs:13`, `scratchpad.rs:18`, `command_palette.rs:19`.
3. Add `mod workspace;` to `main.rs` between `mod win_session;` and `mod worktree;`.

Watch for: a name collision in `app.rs` if it already has a `workspace` binding in scope at module level, and the `worktree` module, which is a different thing and must not be touched.

Commit subject: `refactor(app): move WorkspaceKey to its own module`

## Task 2: move `ShellDecision` and `shell_decision` to a new module

Create `alacritree/src/shell_decision.rs` holding, verbatim with their doc comments:

- `pub enum ShellDecision` (`app.rs:6243`, including its `#[derive(Debug, PartialEq, Eq)]`)
- `pub fn shell_decision(...)` (`app.rs:6256`)

Its imports: `crate::config::Profile` and `crate::wsl::ShellChoice`. Inside the moved body, `crate::config::Profile` in the signature may stay fully qualified or become an import. Either is fine, pick one and be consistent.

The sibling helpers around it in `app.rs`, namely `profile_shell`, `config_session_shell`, `profile_session_shell` and `shimmed_wsl_argv`, **stay in `app.rs`**. They build `alacritty_terminal::tty::Shell` and `wsl_helper::WslProbe`, so moving them would widen the new module to the whole spawn path's dependencies, and each has exactly one caller.

Then:

1. Move the tests. `app.rs`'s test module holds the `shell_decision` tests at roughly lines 16709 to 16806: the `test_profiles()` helper plus the test named `override_profile_wins_over_location_and_default` and the five that follow it. `test_profiles()` is used by nothing else in `app.rs`. Verify that with `rg -n 'test_profiles' alacritree/src/app.rs` before and after. Move all of them into `shell_decision.rs`'s own `#[cfg(test)] mod tests`, fixing the `use` lines the new location needs.
2. `app.rs` keeps its call site at line 2176: add `use crate::shell_decision::{ShellDecision, shell_decision};` so lines 2176 to 2190 still resolve unqualified.
3. Repoint `cli/doctor.rs:22` from `use crate::app::{ShellDecision, shell_decision};` to `use crate::shell_decision::{ShellDecision, shell_decision};`.
4. Add `mod shell_decision;` to `main.rs` between `mod session;` and `mod sidebar_focus;`.

Commit subject: `refactor(app): move shell_decision out of app`

## Task 3: move `harden_dll_search_path` to a new `dll_search` module

Create `alacritree/src/dll_search.rs` holding both arms, verbatim with the long doc comment that currently sits above the `#[cfg(windows)]` arm at `main.rs:98`:

- `#[cfg(windows)] pub fn harden_dll_search_path()`, body unchanged, with the `Once` and `windows_sys` imports staying inside the fn
- `#[cfg(not(windows))] pub fn harden_dll_search_path() {}`

Both become `pub`. They are bare `fn` today only because `main.rs` is the crate root. The doc comment's last paragraph says "`main` does it at startup and every pseudoconsole open repeats it". That sentence stays true and stays as written.

Then repoint all six call sites to `crate::dll_search::harden_dll_search_path()`:

| File | Line | Kind |
| --- | --- | --- |
| `main.rs` | 123 | inside `main`, currently a bare call |
| `session.rs` | 1243 | production, inside `open` |
| `session.rs` | 2738 | test module |
| `grid_gl.rs` | 838 | test module |
| `grid_instances.rs` | 400 | test module |
| `focus_priority/windows.rs` | 584 | test module |

Add `mod dll_search;` to `main.rs` between `mod digest;` and `mod doppler;`.

Leave `attach_parent_console` (`main.rs:264`, no-op twin at `:281`) exactly where it is. It has the same shape and is a natural co-tenant, but it blocks nothing and moving it widens the commit.

Verification note: the non-Windows arm is what CI compiles on Linux and macOS, so `devrun task check` on Windows exercises only the `cfg(windows)` arm. Confirm both arms compile by eye and state in the report that only the Windows arm was machine-checked.

Commit subject: `refactor(main): move harden_dll_search_path to dll_search`

## Task 4: move the notification bridge into a `notify` module directory

The issue framed this item as inverting `notify_click` to a channel. That framing is wrong, and the architecture session confirmed it: the channel already exists as `NOTIFY_TX: OnceLock<Mutex<Sender<SessionId>>>` at `app.rs:59`, set in `AlacritreeApp::new` at `app.rs:1186`. `notify_click` is already just `send` plus `request_repaint`. So this task is a relocation like the other three, only larger.

**It must be a module directory, not a flat file.** Create `alacritree/src/notify/mod.rs` and move `alacritree/src/notify_macos.rs` to `alacritree/src/notify/macos.rs`. A flat `notify.rs` would only relocate the cycle: `notify_worker`'s macOS arm calls `notify_macos::notify`, and that module's delegate calls back into `notify_click`. Absorbing the macOS module as a platform submodule removes the cycle instead. `focus_priority/` is the existing house pattern for this shape, so follow it.

### What moves into `notify/mod.rs`

From `app.rs`, verbatim with their doc comments:

- `static NOTIFY_TX` (line 59) and its doc comment
- `fn latest_notification_click` (line 12438)
- `fn notify_attention` (line 12449)
- `pub(crate) fn notify_click` (line 12470)
- all three `notify_worker` `cfg` arms (lines 12480, 12503, 12522)

Plus a function that owns creating the channel, replacing the two statements at `app.rs:1181` and `app.rs:1186`. Shape it so `app` receives only the `Receiver` while the module keeps the `set` and its existing comment about why the error is ignored:

```rust
pub fn channel() -> Receiver<SessionId> { ... }
```

Declare the platform submodule in `notify/mod.rs` with the same `#[cfg(target_os = "macos")]` gate `main.rs:39` and `main.rs:40` use today.

### Naming inside the new module

Drop the `notify_` prefix where it now repeats the module name, so call sites read as `notify::attention(...)`, `notify::click(...)`, `notify::latest_click(...)`. Keep the private worker as `worker`. This is the only naming change in the whole plan and it is confined to items that are private or `pub(crate)`.

### Edits outside the new module

1. `app.rs:1178`: `crate::notify_macos::init(...)` becomes the `notify::macos::init(...)` path.
2. `app.rs:1181` to `app.rs:1186`: replaced by the `notify::channel()` call. The `notify_rx` field on `AlacritreeApp` and the `from_parts` parameter stay exactly as they are, and the test constructors that build their own `mpsc::channel()` are untouched.
3. `app.rs:9653`: `latest_notification_click(&self.notify_rx)` becomes the new path.
4. `app.rs:9734`: `notify_attention(&self.sessions[idx], ctx)` becomes the new path.
5. `notify/macos.rs:97`: `crate::app::notify_click(...)` becomes the in-module `super::click(...)` or `crate::notify::click(...)`. This is the edge the task exists to delete.
6. `main.rs`: remove the `#[cfg(target_os = "macos")] mod notify_macos;` pair at lines 39 and 40, and add `mod notify;` in its place. Alphabetically it lands in the same slot, between `mod multiplexer;` and `mod panel_filter;`.
7. Move the `latest_notification_click` test, `a_pile_of_notification_clicks_resolves_to_the_newest` at `app.rs:14691` to `app.rs:14701`, into `notify/mod.rs`'s own test module, renamed only where it names the moved function.

### The new dependency to expect

`notify_attention` reads `session.working_directory`, `session.title` and `session.id`, so `notify/` will depend on `crate::session`. That is not a cycle: nothing in `session` reaches back into `notify`. Do not try to avoid it by passing the three fields separately.

Commit subject: `refactor(app): move the notification bridge into notify`

## Done when

- `rg -n 'crate::app::' alacritree/src --glob '*.rs' | rg -v 'src.app\.rs'` prints nothing.
- `rg -n 'crate::harden_dll_search_path' alacritree/src --glob '*.rs'` prints nothing.
- All four verification commands pass.
- Four commits, one per task.
