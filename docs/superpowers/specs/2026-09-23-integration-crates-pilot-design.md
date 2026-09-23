# Integration crates: overall design and the doppler pilot

Issue: #133. Related: #134 (version control trait), #135 (typed errors).

This spec has two parts. The first records the decisions that apply to every
integration as it moves out of `alacritree/src/`. The second is the detailed
design for the first step: a shared foundation crate plus doppler, rebuilt as
a checkout lifecycle hook. Each later integration gets its own spec and plan
and follows the pattern the pilot sets.

## Goals

- The compiler enforces boundaries. An integration cannot reach into app
  internals or into another integration.
- Incremental builds get faster. A change to one backend recompiles that
  backend's crate and the app, not everything.
- Adding a backend means writing one crate that implements a trait, plus a
  registration line in the app.

Reuse outside alacritree is not a goal. Behavior stays the same except where
this spec names a change.

## Part 1: decisions for the whole effort

### Crate model

Three kinds of crate:

- **Foundation.** `alacritree_common` holds what every integration needs:
  process spawning (`command_ext`), the job pool (`jobs`), WSL support (`wsl`,
  `wsl_helper`), and the tool path table (`tools`).
- **One crate per integration type.** It holds the trait, the shared models,
  the error type, a test fake behind a `test-support` feature, and any
  generic backend such as a user-defined command backend.
- **One crate per backend.** For example `alacritree_doppler`,
  `alacritree_herdr`, `alacritree_taskwarrior`. It implements its type's trait
  and owns its config section.

The app (`alacritree/`) is the only crate that depends on specific backends.
It owns each integration type's dispatch enum, its selection or registration
logic, and all egui UI.

Integration types and their current backends:

| Type | Backends today | Planned |
|---|---|---|
| Checkout hooks | doppler, command | mise, direnv, Claude bell |
| Multiplexer | herdr, zellij | |
| Task backend | taskwarrior | command |
| Forge | gh | direct GitHub API |
| Diff viewer | delta, tuicr, custom | |
| Version control | git | jj, mercurial (#134) |

Single-backend types get a trait now because each has a second backend on the
roadmap or a command backend users can configure.

### Layout and naming

- New crates live in `crates/`. The workspace lists them with one glob,
  `crates/*`.
- `alacritree/` stays at the root. Release tooling spells its path in
  `release-please-config.json`, `dist-workspace.toml`, CI, the scoop bucket,
  and the macOS bundle script.
- The vendored crates (`alacritty*`, `egui-winit`) stay where they are so
  upstream syncs remain plain diffs.
- Directory name equals package name, with an `alacritree_` prefix:
  `crates/alacritree_doppler` is the package `alacritree_doppler`.

### Config

- Each crate declares its own raw config types (`Raw*`, `Deserialize`,
  `JsonSchema`, doc comments) and a `resolve` into its runtime type.
- The app's `RawIntegrations` names each section with one field per crate.
  Crates do not register themselves at link time, because schemars needs the
  type tree at compile time.
- Merging `alacritty.toml` and `alacritree.toml` works on `toml::Value` before
  deserialization, so it does not care where a type lives.
- Raw type names stay unique across the workspace. schemars names schema
  definitions after types, and `schema-defaults-allowlist.txt` lists keys as
  `Type.field`.
- Config helpers that several sections share (`ClosedSet`, `RgbStr`,
  `RawIconStyle`) move to `alacritree_common` when the first integration that
  needs them moves, not before.

### Dispatch

- Every integration trait is marked `#[ambassador::delegatable_trait]`. The
  app derives `Delegate` on an enum with one variant per backend. Test fakes
  are variants behind `#[cfg(test)]`, and command backends are ordinary
  variants carrying their config.
- `enum_dispatch` cannot link a trait and an enum that live in different
  crates. It keeps its trait registry in a static inside the proc macro,
  which does not survive between crate compilations
  (`enum_dispatch` v0.3.13 `src/cache.rs:38-42`), and a missed link generates
  no impls and no error. The existing `Multiplexer` enum keeps
  `enum_dispatch` until herdr and zellij move out, then switches.
- A trait cannot use an associated type that differs between backends,
  because ambassador rejects it for enums
  (`tests/compile-fail/enum_associated_types.rs`). Error types are therefore
  one concrete type per integration type.
- `Box<dyn Trait>` is not used for integrations.

A throwaway benchmark (not committed) compared the release assembly of
enum_dispatch in one crate, ambassador in one crate, ambassador across
crates, a hand-written `match` across crates, and `Box<dyn>`. Under both thin
LTO and no LTO, the three enum approaches produced byte-identical code, with
backend bodies inlined. `dyn` could not inline and used about 70% more
instructions in a loop over hooks. The only difference was a non-inlinable
method called across crates. Under thin LTO it goes through a GOT slot rather
than a direct jump. The hand-written match did the same, so the crate
boundary causes it, not ambassador. The benchmark could not measure a cost.

### Closed name sets

`strum` derives cover every closed set of names: `Tool` (`EnumCount`,
`VariantArray`, `IntoStaticStr`, `Display`) and each integration type's kind
enum. `EnumTable` is not used. Its own docs say it may be deprecated
(`strum_macros` 0.26.4 `src/lib.rs:548-551`).

### Errors

New crates report errors with `thiserror` enums, not `String`. `anyhow` is
reserved for top-level code that only prints an error. Migrating the rest of
the app is #135.

### Roadmap

1. Foundation and doppler pilot (this spec).
2. gh and the forge trait.
3. Diff viewer.
4. Multiplexer trait, then herdr and zellij. `Multiplexer` switches to
   ambassador here.
5. Task backend trait, taskwarrior, and a command backend. The taskwarrior
   filter and modifier strings in `tasks/view.rs` and `tasks/tree.rs` become
   typed operations.
6. Version control, after #134.

## Part 2: the pilot

### Scope

- Create `alacritree_common`, `alacritree_checkout_hooks`, and
  `alacritree_doppler`.
- Replace doppler's three direct call sites with the checkout hook list.
- Add the user-defined command hook.
- Run doppler in the checkout's WSL distro when the checkout is in WSL. This
  is a behavior change: today doppler does nothing useful for WSL worktrees.
- Extend CI and lint coverage to `crates/`.

Out of scope:

- Moving the Claude Code bell step (`worktree.rs:181`) into a hook.
- Running hooks when a worktree whose directory is already gone is pruned
  (`prune_worktree`). Today doppler cleanup does not run there, and the pilot
  keeps that.
- Changing the release LTO profile.

### Crate graph

```
alacritree (app)
 ├─ alacritree_doppler
 │   └─ alacritree_checkout_hooks
 │       └─ alacritree_common
 ├─ alacritree_checkout_hooks
 └─ alacritree_common
```

### alacritree_common

Moves from `alacritree/src/` unchanged except where noted:

- `command_ext.rs`, `jobs.rs`, `wsl.rs`, `wsl_helper.rs`, `tools.rs`.
- The `wsl_helper.rs` test that builds a command through
  `multiplexer::Side` (`wsl_helper.rs:1602`) is rewritten against a plain
  command, removing the only reference back into the app.
- `Tool` gains `strum` derives. `VariantArray` replaces `Tool::ALL`, and
  `IntoStaticStr`/`Display` with `serialize_all = "lowercase"` replace
  `name()`. The path table becomes `[ToolPaths; Tool::COUNT]`, indexed by
  `tool as usize`. `Tool`'s `serde::Serialize` output does not change.
- `HELLO_TOOLS` stays a hand-kept list, because it includes `zellij`, which
  is not a `Tool`. A new test asserts every `Tool` name appears in it.
- A side-aware runner that the hook crates share:

```rust
pub enum Side<'a> { Native, Wsl { distro: &'a str } }

/// Where a program runs for `path`: the native side, or the distro that
/// holds a `\\wsl.localhost\<distro>\...` path.
pub fn side_of(path: &Path) -> Side<'_>;
```

The runner resolves the program for that side. Natively it is the configured
path. In a distro it is the configured `wsl_path`, then the resident
helper's lookup, then the distro's login shell. A Windows binary is never
used for a WSL checkout.

The alacritree crate re-exports the moved modules from its own root for now.
Call sites change their imports as they are touched, not all at once.

### alacritree_checkout_hooks

```rust
#[ambassador::delegatable_trait]
pub trait CheckoutHook {
    /// alacritree just created `event.checkout` as a worktree of `event.main`.
    fn on_created(&self, event: &Checkout, blocking: &Blocking) -> Outcome { Ok(None) }
    /// This process opened its first shell in a linked worktree. Fires again
    /// after a restart, so implementations must be idempotent.
    fn on_opened(&self, event: &Checkout, blocking: &Blocking) -> Outcome { Ok(None) }
    /// The worktree at `event.checkout` was removed. The path was resolved
    /// before git deleted the directory.
    fn on_removed(&self, event: &Checkout, blocking: &Blocking) -> Outcome { Ok(None) }
}

pub struct Checkout<'a> { pub main: &'a Path, pub checkout: &'a Path }

/// A line for the progress UI, or nothing when the hook had nothing to do.
pub type Outcome = Result<Option<String>, HookError>;

#[derive(Debug, thiserror::Error)]
pub enum HookError {
    #[error("{program} failed ({status}): {stderr}")]
    Failed { program: String, status: std::process::ExitStatus, stderr: String },
    #[error("could not run {program}")]
    Spawn { program: String, #[source] source: std::io::Error },
}

/// A set of hooks that runs every event on each member in order.
pub trait CheckoutHooks {
    fn created(&self, event: &Checkout, blocking: &Blocking) -> Vec<Outcome>;
    fn opened(&self, event: &Checkout, blocking: &Blocking) -> Vec<Outcome>;
    fn removed(&self, event: &Checkout, blocking: &Blocking) -> Vec<Outcome>;
}
```

A missing program is not an error. The hook returns `Ok(None)` and logs at
debug level. A program that runs and exits non-zero is `HookError::Failed`.

The crate also holds:

- `CommandHook`, the user-defined hook described below.
- `FakeHook` behind the `test-support` feature. It records every event it
  receives and returns scripted outcomes.

#### The command hook

```toml
[integrations.checkout_hooks.command.mise]
enabled    = true
path       = "mise"
wsl_path   = ""
on_created = ["trust", "{checkout}"]
on_opened  = ["trust", "{checkout}"]
on_removed = []
```

- Hooks are named tables, not an array. Array merging concatenates, which
  would make a hook defined in `alacritty.toml` impossible to override or
  disable from `alacritree.toml`.
- `{checkout}` and `{main}` are the placeholders, written in the same style
  as the custom diff viewer's `{file}` and `{base}`. In a distro they expand
  to Linux paths.
- An empty template skips that event.
- The command runs on the checkout's side. Its working directory is the
  checkout, or the main checkout for `on_removed`.
- In a distro without a configured `wsl_path`, the program runs through the
  login shell, as custom diff viewers do. Exit code 127 from the shell means
  the program was not found and the hook is skipped. Any other non-zero exit
  is `Failed` with the first line of stderr.
- Exit 0 reports `Ran <name>`. Standard output is ignored.

### alacritree_doppler

- Holds today's `doppler.rs` logic behind `DopplerHook`, which implements
  `on_created` and `on_opened` as mirroring and `on_removed` as forgetting.
- Reports `Linked N Doppler scope(s)` and `Dropped N Doppler scope(s)` as its
  outcome lines, or `None` when N is 0.
- Stays silent when doppler is missing or has nothing to do, as today.
- Runs on the checkout's side. For a WSL checkout it runs the distro's
  doppler, with scope paths translated through `wsl::windows_to_linux`, so
  scopes land in the distro's own doppler config.
- Owns `RawDoppler`: `path`, `wsl_path`, and a new `enabled` defaulting to
  `true`. It replaces the `raw_tool_table!(RawDoppler, "doppler")` line.

### App changes

A new `alacritree/src/checkout_hooks.rs`:

```rust
#[derive(Delegate)]
#[delegate(CheckoutHook)]
enum Hook {
    Doppler(DopplerHook),
    Command(CommandHook),
    #[cfg(test)]
    Fake(FakeHook),
}

/// Built-in hooks first, then command hooks sorted by name.
pub(crate) struct Hooks(Vec<Hook>);

impl Hooks {
    pub(crate) fn from_config(config: &IntegrationsConfig) -> Self;
}

impl CheckoutHooks for Hooks { /* each event on each hook, in order */ }
```

`Hooks` is rebuilt from the current config wherever an event fires, so a
config reload takes effect on the next event.

Call sites:

| Today | After |
|---|---|
| `worktree.rs:173`, in `create` | `hooks.created(..)`. Each `Ok(Some(line))` is a progress step; each error is a progress step naming the hook |
| `worktree.rs:587`, in `delete_worktree` | `hooks.removed(..)`, each outcome logged |
| `app.rs:1140`, `sync_doppler_scopes` | `sync_checkout_hooks` runs `hooks.opened(..)` on a background job; `doppler_synced` becomes `hooks_opened` |

`worktree::create` and `delete_worktree` take `hooks: &impl CheckoutHooks`
rather than the app's `Hooks`, so the git code keeps no dependency on the
app's dispatch enum when it moves to its own crate. Their three callers build
`Hooks` from resolved config: the GUI modal (`app/modals.rs:988`), IPC
(`ipc/server.rs:244`), and the offline CLI (`cli/offline.rs:153`, with config
loaded at `cli/mod.rs:542`).

### Config and schema

| Type | Crate | Table |
|---|---|---|
| `RawDoppler` | `alacritree_doppler` | `[integrations.doppler]` |
| `RawCheckoutHooks` | `alacritree_checkout_hooks` | `[integrations.checkout_hooks]` |
| `RawCommandHook` | `alacritree_checkout_hooks` | `[integrations.checkout_hooks.command.<name>]` |

- Defaults live in each type's `Default` impl. `enabled` is `true`, and the
  `on_*` templates and `wsl_path` are empty.
- `RawCommandHook.path` has no default and is required. It is the one new
  line in `schema-defaults-allowlist.txt`.
- `schema/alacritree-config.json` and `docs/config-reference.md` are
  regenerated with
  `ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema`.

### Build, CI and lints

- The root `Cargo.toml` gains `crates/*` in `members`, and `ambassador` and
  `thiserror` as workspace dependencies.
- CI build, test and clippy (`ci.yml:30,33,46`) switch from `-p alacritree`
  to `--workspace` with the vendored crates excluded.
- `crates/clippy.toml` carries the same `disallowed-methods` list as
  `alacritree/clippy.toml`, because clippy reads the nearest config walking up
  from a crate. Each file has a comment pointing at the other.
- The UI-thread audit (`alacritree/tools/ui-thread-audit.py:54`, which
  hardcodes `alacritree/src`) also scans `crates/*/src`.
- The PR confirms that release-please's `cargo-workspace` plugin accepts new
  unpublished workspace members that are missing from `packages`.
- AGENTS.md describes `crates/`, the crate model, and the thiserror and
  ambassador conventions.

### Tests

| Crate | Coverage |
|---|---|
| `alacritree_common` | Moved tests move with their modules. The `wsl_helper` test is rewritten without `multiplexer::Side`. New: `Tool` strum names match the old `name()`, `Tool::COUNT` matches `VARIANTS`, every `Tool` is in `HELLO_TOOLS`, and `side_of` handles native paths, `\\wsl.localhost\` paths and `\\wsl$\` paths. |
| `alacritree_checkout_hooks` | Command hook: placeholder expansion, empty template skips, exit 0 reports `Ran <name>`, exit 127 skips, other non-zero is `Failed` with the first stderr line, Linux path expansion for a WSL checkout. Unix tests use `sh -c` scripts. |
| `alacritree_doppler` | Existing `rebase_scope` tests. Side selection and path translation as pure functions. Mirror and forget end to end on Unix against a fake `doppler` script set through the tool path. |
| App | `Hooks` dispatches through ambassador across crates, runs hooks in order, continues after a failure, and returns every outcome. `worktree::create` and `delete_worktree` deliver events to a recording `FakeHook` (using `test_util::init_repo` and `add_worktree`). `sync_checkout_hooks` fires `on_opened` once per worktree per process. |
| Existing | `config_schema`, `schema_defaults` and `steady_state` pass after regeneration, and the allowlist diff is the one new line. |

### Order of work

1. Workspace and CI plumbing, including `crates/clippy.toml` and the audit
   path.
2. `alacritree_common` with the moved modules and the `Tool` strum change.
   The app re-exports it and everything passes unchanged.
3. `alacritree_checkout_hooks` with the trait, the error type and the fake.
   First, a cross-crate `Delegate` derive compiles in the app. That proves
   ambassador works across these crates before anything is built on it.
4. `alacritree_doppler`, native behavior only. Call sites switch to `Hooks`.
5. The WSL side rule for doppler, as its own commit.
6. The command hook and its config.
7. Schema regeneration and docs.

### Delivery

The pilot's PR, and every later PR in this effort, is opened against the
upstream repository `mathix420/alacritree` (Arnaud Gissinger's, the one
`alacritree/Cargo.toml` names as `repository`), targeting its default
branch. The head branch lives on the `AbysmalBiscuit/alacritree` fork. No PR
for this work is opened against the fork itself.

### Risks

- **The release-please `cargo-workspace` plugin.** Its handling of new
  members that are missing from `packages` is unverified. If it rejects them,
  either list the crates with `skip-github-release` or configure the plugin to
  ignore them.
- **Re-exports hiding the boundary.** While the app re-exports common modules,
  new code could keep importing through `crate::`. Call sites switch to the
  crate paths as they are touched, and the last roadmap step deletes whatever
  re-exports remain.
- **WSL doppler.** No WSL machine runs in CI. The side selection and path
  translation are pure functions with unit tests, and the spawn goes through
  the same helper as other WSL tools. A manual check on a Windows host with a
  WSL worktree is part of the PR's test plan.
