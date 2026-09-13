# Configurable diff viewer and shared tool lookup

Issue: AbysmalBiscuit/alacritree#82. Follow-up for pushing reviews to agents: #95.

## Problem

The git panel opens every diff in delta, wired in as git's `core.pager`. A user who reviews with another tool, such as tuicr, has no way to use it from the panel. The panel also offers no way to review a whole section at once, only one file per click.

Separately, alacritree looks up external programs inside WSL distros in three places that each keep their own list: the resident helper's hello probes git, delta and gh; `cli/doctor.rs` probes git, gh, delta and doppler through `WSL_TOOLS`; and `wsl::discover_delta` probes delta alone. A new viewer tool would add a fourth.

## Goals

- The user picks the diff viewer in config: a built-in preset (`delta`, `tuicr`) or a custom command.
- File rows keep opening a per-file diff, in whatever viewer is configured.
- Each git panel section header can carry a review button that opens the whole section in the viewer. The buttons are opt-in.
- Each section review is also a palette Action.
- One registry of external tools, probed once per WSL distro, serves every caller that resolves a tool inside a distro.
- Arnaud's workflow is unchanged with no config: delta, no buttons.

## Non-goals

- Sending review output to agents. Agents read tuicr comments through `tuicr review comments` and tuicr's own skill. Push forwarding is #95.
- Overriding the git library. `git2` calls stay as they are; `[integrations.git] path` governs only `git` processes alacritree spawns by name. Batch scripts that run inside a WSL distro keep finding `git` on that distro's PATH through the resident helper's plain `sh`.

## Config

Everything lives in `alacritree.toml` under `[integrations]`, next to the existing `[integrations.herdr]`. Every key has a schema default, kept in the `Raw*` type's `Default` impl.

A setting that exists only because of a tool lives in that tool's table as a flat key, appearance included. `[ui]` keeps app-wide settings, including ones that mention a tool in passing. This PR follows that rule for the keys it adds. Moving the existing tool-specific `[ui]` keys is #96.

```toml
[integrations.git]
path = "git"
wsl_path = ""

[integrations.gh]
path = "gh"
wsl_path = ""

[integrations.doppler]
path = "doppler"
wsl_path = ""

[integrations.herdr]
path = "herdr"            # joins the existing herdr keys
wsl_path = ""

[integrations.delta]
path = "delta"
wsl_path = ""

[integrations.tuicr]
path = "tuicr"
wsl_path = ""

[integrations.diff_viewer]
preset = "delta"          # "delta" | "tuicr" | "custom"
section_buttons = false
button_icon = "review"    # glyph or label drawn on each section header

[integrations.diff_viewer.custom]
# Pager mode: git runs the panel's own diff with this as core.pager.
pager = ""
wsl_pager = ""
# Direct mode: path plus one argv template per row kind and per section.
path = ""
wsl_path = ""
staged = []
unstaged = []
untracked = []
branch = []
staged_scope = []
unstaged_scope = []
branch_scope = []
```

### Tool paths

Each tool has one path per side, because a Windows path means nothing inside a distro.

- `path` is the native program. It defaults to the tool's name, which the OS finds on `PATH`; any other value runs as written.
- `wsl_path` is the program inside every WSL distro. It defaults to empty, which finds the tool by name through the tool registry; any other value runs as written and is never probed, because `wsl::probe_tools` interpolates program names into a shell script and only registry literals may reach it.

### `[ui] delta_path`

It stays readable and is marked deprecated in its doc comment, the same treatment `[ui.wsl] automount_root` got. It was one path used on both sides, so it keeps filling each side that `[integrations.delta]` leaves at its default: `path` while it is `"delta"`, `wsl_path` while it is empty. Filling either side logs a `log::warn!` naming the new keys. An existing config therefore behaves as it did.

Dropping the key outright would silently lose the override, because the raw config structs do not deny unknown fields.

### Custom validation

`preset = "custom"` needs exactly one mode: a non-empty `pager`, or a non-empty `path` together with the templates. Anything else logs a warning and resolves to the delta preset, matching how `[integrations.herdr]` handles unknown values.

`wsl_pager` and `wsl_path` pair with `pager` and `path` for WSL workspaces. Empty, the native value runs through the distro's login shell, which finds a bare name; set, it runs as written.

A direct-mode template left empty makes that row kind or section unavailable: a click on its rows opens nothing and its header shows no button.

### Placeholders

Templates substitute `{file}` (the row's path) and `{base}` (the resolved base ref for `Changes vs`). Each placeholder is substituted inside its own argv element, so a file name is never shell-parsed. Scope templates have no `{file}`. A branch row or `branch_scope` with no resolved base stays unclickable, as branch rows already do.

## Presets

Every preset is a value of the same type as `custom`, so the built-in viewers and user configs take one code path.

| Target | delta (pager mode) | tuicr (direct mode) |
|---|---|---|
| Staged row | `git diff --cached -- {file}` | `-w -p {file}` |
| Unstaged row | `git diff -- {file}` | `-w -p {file}` |
| Untracked row | `git diff --no-index -- /dev/null {file}` | `-w -p {file}` |
| Branch row | `git diff {base}... -- {file}` | `-r {base}...HEAD -p {file}` |
| Staged header | `git diff --cached` | `-w` |
| Unstaged header | `git diff` | `-w` |
| Changes vs header | `git diff {base}...` | `-r {base}...HEAD` |

tuicr's revision parser accepts `A...B` as a merge-base range, so its branch targets match the panel's triple-dot semantics. tuicr has no way to split staged from unstaged, so both of its uncommitted targets open the same view. Its `-w` diff includes untracked files, so `-p {file}` works for untracked rows too. tuicr saves a comment to its session file when the comment is submitted, so closing the pane through a toggle click loses at most a comment still being typed. `git diff` in the delta unstaged header does not list untracked files; that matches plain git and the rows still cover them.

## Architecture

The work builds on the branch of PR #227, which already carries the #71 split: the git panel lives in `app/git_panel.rs` and owns its state in `GitPanel`. Two new modules sit at crate level. Both are consumed from outside `app/`: `config.rs` holds the resolved viewer and `cli/doctor.rs` uses the tool registry, and nothing outside `app/` may reference `crate::app`.

Neither module adds a module cycle. `tools` imports `wsl`, `wsl_helper` and `jobs`. `diff_viewer` imports only `tools::Tool`. `config` imports both. `wsl_helper` names tools by program name and never imports `tools`.

### `tools.rs`

The registry of external programs and their WSL resolution.

- `enum Tool { Git, Gh, Delta, Doppler, Herdr, Tuicr }` with `name()` returning the literal program name and `ALL`.
- `ToolPaths { native, wsl }` per tool, published once at startup by `configure`, the same way `wsl::set_automount_root` and `wsl_helper::set_enabled` are. The spawn sites that need a path (`pr_status`, `doppler`, `worktree`, herdr) hold no `Config`, so the registry is the one place they read it from. `main.rs` and `doctor` call `configure` after loading config.
- `program(Tool) -> String`: the native path, which is the bare name unless set. Native spawns use it.
- `wsl_program(Tool) -> String`: the WSL path when set, otherwise the bare name. Spawns inside a distro where a shell finds the program use it.
- `wsl_resolved(Tool, distro, on_found) -> Option<String>`: for the UI thread. It returns the override or a cached path, and otherwise starts at most one background lookup per distro and tool and returns `None`. The lookup asks the resident helper's hello first, then probes live, and calls `on_found` so the caller repaints. A miss is never cached, so a mid-session install is picked up on a later lookup. This absorbs the adopt-then-spawn logic `wsl_delta_path` held.
- `wsl_in_job(Tool, distro, &Blocking) -> String`: the same answer for a pool job, which may reach the helper directly and falls back to the bare name.

Callers that move onto it:

- `wsl_helper.rs`: the hello probes every registry name in `HELLO_TOOLS` instead of git, delta and gh, and `capability(distro, program)` replaces `capability_delta` and `capability_gh`. The field count changes, so `PROTOCOL_VERSION` goes to 2. A test pins `HELLO_TOOLS` to `Tool::ALL`.
- `wsl.rs`: `discover_delta` goes away. `probe_tools` stays as the probe primitive.
- `cli/doctor.rs`: `WSL_TOOLS` becomes `Tool::ALL`. Consequence text per tool stays in doctor. Native binary checks look up the configured path, and doctor adds an optional check for the program the configured viewer runs.
- `pr_status.rs`: native `gh` spawns use `program`, the WSL one uses `wsl_in_job`.
- `herdr/cli.rs`, `multiplexer.rs` and `app/herdr_glue.rs`: `PROGRAM` becomes `herdr::program(side)`.
- `worktree.rs` and `doppler.rs`: each spawn of a registry tool takes `program`, or `wsl_program` inside a distro, instead of a literal name.
- `GitPanel` loses `wsl_delta_paths` and `pending_delta`.

### `diff_viewer.rs`

Pure: no egui, no app state.

- `enum Program { Tool(Tool), Custom { path, wsl_path } }`: a registry tool resolved at spawn time, or a custom viewer's native value with its optional WSL counterpart.
- `enum Viewer { Pager { pager, args }, Direct { program, templates } }` with `Viewer::delta()` and `Viewer::tuicr()` as the presets, and `Templates` holding one argv list per row kind and per section.
- `DiffRequest`, `DiffSource`, `diff_key` and `diff_args`, moved out of `app/git_panel.rs`.
- `enum Target { Row(DiffRequest), Section(Section) }`, where `Section` is staged, unstaged or branch with its base, and `Target::key()` names the pane.
- `opens(&Viewer, &Target) -> bool` and `plan(&Viewer, &Target) -> Option<Launch>`, which returns `None` when the target is unavailable for this viewer.
- `enum Launch { Pager { pager, pager_args, git_args }, Direct { program, args } }`.
- The native and WSL builders for both launch kinds. A pager launch passes git, the pager command and the diff arguments to one `sh` script as positional parameters, which exports `LESS` where git runs; before the pager's path is known the same script runs through the login shell. A direct launch uses `wsl.exe --cd <workspace> --exec <resolved> <args>` when the path is known, and otherwise re-execs through the login shell with the program passed as a positional parameter.

### `config.rs`

`IntegrationsConfig` gains `git`, `gh`, `doppler`, `delta` and `tuicr` tool tables with `path` and `wsl_path`, and `diff_viewer`; `HerdrConfig` gains `path` and `wsl_path`. `IntegrationsConfig::tool_paths()` is what `tools::configure` takes. `DiffViewerPreset` is the closed set for `preset`, and `DiffViewerConfig` holds the resolved `Viewer`, `section_buttons` and `button_icon`. `Config::delta_path` goes away; its precedence moves into resolving `[integrations.delta] path`.

### `app/git_panel.rs`

- `show_git_sidebar` draws `button_icon` as a review button right-aligned on each section header when `section_buttons` is on and `plan` returns `Some` for that section. The button reads as active while its section's pane is open, the same cue rows use.
- `open_diff` takes a `Target`, asks `diff_viewer::plan`, resolves the program through `tools` inside WSL, and spawns the `SessionKind::Diff` session. Toggle semantics do not change: one diff pane per workspace, the same target closes it, a different target replaces it. Section keys are `section:staged`, `section:unstaged` and `section:branch`, so they never collide with row keys.

### Actions

`bindings.rs` gains `NamedAction::ReviewStaged`, `ReviewUnstaged` and `ReviewBranch`, listed by `bindable_actions()` so the palette offers them regardless of `section_buttons`. `dispatch_git_action` in `app/git_panel.rs` routes each to `open_diff` with its section target. An action whose section is unavailable does nothing.

## Error handling

- A spawn failure opens the error dialog, as `open_diff` does today.
- A missing viewer binary behaves like a missing delta does now: the pane shows the spawn or shell error. `doctor` reports it.
- An invalid custom config warns once at load and falls back to delta.

## Testing

- `diff_viewer.rs`: each preset renders the expected argv for every target; placeholders substitute per element and a file name containing spaces or quotes stays one argument; empty custom templates make targets unavailable; invalid custom falls back to delta; WSL wrapping for direct launches, resolved and login-shell.
- `config.rs`: tool paths default to their names and WSL discovery, and a blank value means the default; the deprecated delta key fills each side left at its default; presets, an unknown preset, and custom validation including the WSL values.
- `wsl_helper.rs`: hello parsing at protocol 2, and the script's probe list matching `HELLO_TOOLS`.
- `tools.rs`: `HELLO_TOOLS` matches `Tool::ALL`; a lookup runs once per distro and tool and keeps its hit; a miss is not kept; a landed lookup calls `on_found`.
- `cli/doctor.rs`: the per-distro report covers every registry tool; the viewer check skips a custom pager and warns on a missing program.
- `app/git_panel.rs`: an app built with `AlacritreeApp::from_parts` and an unspawned diff session shows that choosing the open section closes its pane and that a section the viewer cannot open leaves the pane alone; the Review actions map to their sections.
- Regenerate the schema and stock config with `devkit run task test --env ALACRITREE_UPDATE_SCHEMA=1 --env ALACRITREE_UPDATE_STOCK=1`; `config_schema`, `schema_defaults` and `the_stock_config_is_unchanged` pass on a plain run.

## Delivery

One PR closing #82, including the tool registry. The branch stacks on PR #227 per the `[n]` stacking rule. The implementation plan is `docs/superpowers/plans/2026-09-13-diff-viewer-integrations.md`.
