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
- Overriding the git library. `git2` calls stay as they are; `[integrations.git] path` governs only spawned `git` processes.

## Config

Everything lives in `alacritree.toml` under `[integrations]`, next to the existing `[integrations.herdr]`. Every key has a schema default, kept in the `Raw*` type's `Default` impl.

A setting that exists only because of a tool lives in that tool's table as a flat key, appearance included. `[ui]` keeps app-wide settings, including ones that mention a tool in passing. This PR follows that rule for the keys it adds. Moving the existing tool-specific `[ui]` keys is #96.

```toml
[integrations.git]
path = "git"

[integrations.gh]
path = "gh"

[integrations.doppler]
path = "doppler"

[integrations.herdr]
path = "herdr"            # joins the existing herdr keys

[integrations.delta]
path = "delta"

[integrations.tuicr]
path = "tuicr"

[integrations.diff_viewer]
preset = "delta"          # "delta" | "tuicr" | "custom"
section_buttons = false
button_icon = "review"    # glyph or label drawn on each section header

[integrations.diff_viewer.custom]
# Pager mode: git runs the panel's own diff with this as core.pager.
pager = ""
# Direct mode: path plus one argv template per row kind and per section.
path = ""
staged = []
unstaged = []
untracked = []
branch = []
staged_scope = []
unstaged_scope = []
branch_scope = []
```

### Tool paths

A tool's `path` defaults to its own name. When `path` equals that name, alacritree resolves it: from `PATH` natively, through the tool registry inside WSL. Any other value is used verbatim on both sides and is never probed, because `wsl::probe_tools` interpolates program names into a shell script and only registry literals may reach it.

### `[ui] delta_path`

It stays readable and is marked deprecated in its doc comment, the same treatment `[ui.wsl] automount_root` got. Resolution order for delta:

1. `[integrations.delta] path` when it differs from `"delta"`.
2. `[ui] delta_path` when set, with a `log::warn!` naming the new key.
3. `"delta"`.

Dropping the key outright would silently lose the override, because the raw config structs do not deny unknown fields.

### Custom validation

`preset = "custom"` needs exactly one mode: a non-empty `pager`, or a non-empty `path` together with the templates. Anything else logs a warning and resolves to the delta preset, matching how `[integrations.herdr]` handles unknown values.

A direct-mode template left empty makes that row kind or section unavailable: its rows stay unclickable and its header shows no button.

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

The work builds on the #71 refactor branch, where the git panel lives in `app/git_panel.rs` and owns its state in `GitPanel`. Two new modules sit at crate level. Both are consumed from outside `app/`: `config.rs` holds the resolved viewer and `cli/doctor.rs` uses the tool registry, and nothing outside `app/` may reference `crate::app`.

### `tools.rs`

The registry of external programs and their WSL resolution.

- `enum Tool { Git, Gh, Delta, Doppler, Herdr, Tuicr }` with `name()` returning the literal program name and `ALL`.
- A per-distro cache of resolved absolute paths. A miss is never cached, so a mid-session install is picked up on a later lookup.
- A non-blocking lookup for the UI thread that returns the cached path or `None` and starts one background probe per distro and tool when none is in flight, requesting a repaint when it lands. This absorbs the adopt-then-spawn logic that `wsl_delta_path` holds today.
- A blocking probe of `Tool::ALL` for `doctor`.
- `resolve(&Config, Tool, side) -> String`: the configured `path` when it differs from the tool's name, otherwise the name natively or the cached WSL path. Every spawn of a registry tool goes through it, so an override applies everywhere that tool runs.

Callers that move onto it:

- `wsl_helper.rs`: the hello probes every `Tool::ALL` name instead of git, delta and gh, and seeds the cache. The field count changes, so `PROTOCOL_VERSION` goes up. `capability_delta` and `capability_gh` go away.
- `wsl.rs`: `discover_delta` goes away. `probe_tools` stays as the probe primitive.
- `cli/doctor.rs`: `WSL_TOOLS` becomes `Tool::ALL`. Consequence text per tool stays in doctor. Natively, doctor adds an optional check for the tool the configured viewer needs.
- `pr_status.rs`: the WSL `gh` resolution reads the registry.
- `herdr/cli.rs` and `multiplexer.rs`: `PROGRAM` goes away; herdr launches take the resolved path.
- `projects.rs`, `worktree.rs`, `pr_status.rs`, `doppler.rs`, `cli/doctor.rs` and the app's own `git` spawns: each `command_ext::hidden` call for a registry tool takes the resolved path instead of a literal name.
- `GitPanel` loses `wsl_delta_paths` and `pending_delta`.

### `diff_viewer.rs`

Pure: no egui, no app state.

- The resolved config types: `Preset`, `Viewer` (pager or direct, with its templates), built from `[integrations.diff_viewer]` plus the tool paths.
- `DiffRequest`, `DiffSource`, `diff_key`, `diff_args` and the WSL command builders, moved out of `app/model.rs`.
- `enum Target { Row(DiffRequest), Section(Section) }`, where `Section` is staged, unstaged or branch with its base.
- `plan(&Viewer, &Target) -> Option<Launch>`, returning `None` when the target is unavailable for this viewer.
- `enum Launch { Pager { pager, git_args }, Direct { program, args } }`.
- The WSL wrapping for both launch kinds. Pager launches keep the existing direct and login-shell builders and their `LESS` export. Direct launches use `wsl.exe --cd <workspace> --exec <resolved> <args>` when the path is resolved, and otherwise re-exec through the login shell with the program passed as a positional parameter.

### `config.rs`

`IntegrationsConfig` gains `delta`, `tuicr` and `diff_viewer`. `Config::delta_path` goes away; its resolution moves into building the `Viewer`.

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
- `config.rs`: the delta path precedence, including the deprecated key and its warning.
- `tools.rs`: hello parsing at the new protocol version; a miss is not cached; the non-blocking lookup starts one probe per distro and tool.
- `app/git_panel.rs`: with `GitPanel` constructible without an eframe window, a section target toggles its pane and a row target replaces it.
- Regenerate the schema with `ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema`; `config_schema` and `schema_defaults` pass.

## Delivery

One PR closing #82, including the tool registry. The branch stacks on the #71 refactor PR once that is open, per the `[n]` stacking rule.
