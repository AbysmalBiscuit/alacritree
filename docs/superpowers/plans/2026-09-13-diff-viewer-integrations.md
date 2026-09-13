# Configurable diff viewer and shared tool lookup implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The git panel opens diffs in a configurable viewer (delta, tuicr, or a custom command), can review a whole section at once, and every external tool alacritree runs resolves through one registry.

**Architecture:** A crate-level `tools.rs` names every external program (`Tool`), holds the configured path per tool, and resolves a tool's absolute path inside a WSL distro once per distro. A crate-level, egui-free `diff_viewer.rs` turns a viewer plus a target (one row or one section) into a launch and builds its native or WSL argv. `config.rs` resolves `[integrations.<tool>] path` and `[integrations.diff_viewer]` into those types, and `app/git_panel.rs` asks `diff_viewer::plan` before spawning the pane.

**Tech Stack:** Rust 2024, egui/eframe, serde + schemars config, strum closed sets, the `jobs` pool, nextest.

**Spec:** `docs/superpowers/specs/2026-09-13-diff-viewer-integrations-design.md`

## Global constraints

- Only `alacritree/` and `docs/` change. Vendored crates are read-only.
- Worktree: `C:/Users/Lev/Git/github/alacritree-worktrees/feat-ux-configurable-diff-viewer`, branch `feat-ux-configurable-diff-viewer`, based on `upstream/refactor-config-bindings-parse-configuration` (PR #227). Every command below runs from that worktree root.
- With no config, behavior is unchanged: delta through git's `core.pager`, no section buttons, the same pane toggling.
- Every new config key carries its default in its `Raw*` type's `Default` impl under `#[serde(default)]` and a doc comment. Regenerate with `devkit run task test --env ALACRITREE_UPDATE_SCHEMA=1 --env ALACRITREE_UPDATE_STOCK=1`, then run `devkit run task test` again without the env vars and require `config_schema`, `schema_defaults` and `the_stock_config_is_unchanged` to pass.
- Nothing outside `app/` references `crate::app`. The new modules must not create a module cycle: `tools` may import `wsl`, `wsl_helper` and `jobs`; `diff_viewer` imports only `tools::Tool`; `config` may import both. `wsl_helper` must not import `tools`.
- Spawn children through `command_ext::hidden`, never `Command::new`.
- `devkit run task test` takes no test filter. For a focused run use the same argv it renders: `cargo nextest run -p alacritree --locked <filter>`.
- Format with `devkit run task fmt` and lint with `devkit run task clippy` before each commit.
- Claim a file before editing it: `DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm acquire <abs path> --note "issue 82"`.
- Conventional Commits, subject 50 chars or fewer, body wrapped at 72, trailer `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.
- Comments explain a non-obvious why, in the present tense, with no task, issue or PR references, and no em dashes, ellipsis characters or arrow characters in new prose.
- Do not integrate into `all-features`: this work stacks on the #70 refactor.

## File map

| File | Change | Responsibility after the change |
|---|---|---|
| `alacritree/src/wsl_helper.rs` | modify | Hello protocol 2 resolves every registry tool by name; `capability(distro, program)` |
| `alacritree/src/tools.rs` | create | `Tool`, configured paths, per-distro WSL lookups |
| `alacritree/src/diff_viewer.rs` | create | Diff targets, viewers, `plan`, native and WSL argv builders |
| `alacritree/src/config.rs` | modify | `[integrations.<tool>] path`, `[integrations.diff_viewer]`, `[ui] delta_path` deprecation |
| `alacritree/src/main.rs` | modify | `mod tools; mod diff_viewer;`, `tools::configure` at startup |
| `alacritree/src/app/git_panel.rs` | modify | `open_diff(Target)`, section review buttons, Review actions |
| `alacritree/src/wsl.rs` | modify | `discover_delta` removed |
| `alacritree/src/pr_status.rs`, `doppler.rs`, `worktree.rs`, `herdr/cli.rs`, `herdr/mod.rs`, `multiplexer.rs`, `app/herdr_glue.rs` | modify | Spawn registry tools through `tools` |
| `alacritree/src/cli/doctor.rs` | modify | WSL probe over `Tool::ALL`, configured native paths, diff viewer check |
| `alacritree/src/bindings.rs`, `command_palette.rs` | modify | `ReviewStaged`, `ReviewUnstaged`, `ReviewBranch` |
| `schema/alacritree-config.json`, `alacritree/tests/stock-config.json` | regenerate | Published schema and stock config |
| `docs/alacritree.md`, `docs/keyboard-shortcuts.md` | modify | User docs for the new keys and actions |

---

### Task 1: Helper hello resolves every registry tool

The resident helper's hello line carries one path per tool. Today it carries git, delta and gh in fixed struct fields. After this task it carries a positional list named by `HELLO_TOOLS`, and callers ask for a tool by program name.

**Files:**
- Modify: `alacritree/src/wsl_helper.rs:10-65` (protocol, `Capabilities`, `parse_hello`), `:154-165` (`HELPER_SCRIPT` hello), `:765-771` (capability fns), tests `:863-978`, `:1412-1413`
- Modify: `alacritree/src/wsl.rs:573` (`discover_delta` caller)
- Modify: `alacritree/src/pr_status.rs:667`

**Interfaces:**
- Produces: `pub const wsl_helper::HELLO_TOOLS: [&str; 6] = ["git", "gh", "delta", "doppler", "herdr", "tuicr"]`, `Capabilities::path(&self, program: &str) -> Option<&str>`, `pub fn wsl_helper::capability(distro: &str, program: &str) -> Option<String>`. `capability_delta` and `capability_gh` are gone.

- [ ] **Step 1: Write the failing tests**

In `wsl_helper.rs` tests, replace `HELLO_LINE`, `parses_hello_with_missing_tools`, `rejects_unknown_hello_version` and `hello_with_empty_trailing_field_still_parses` with the versions below, and add `the_hello_probes_every_tool_it_reports_in_order`:

```rust
    /// A hello `parse_hello` accepts: protocol 2, every tool field and the
    /// runtime dir empty.
    const HELLO_LINE: &str = "hello\t2\t\t\t\t\t\t\t\n";
```

```rust
    #[test]
    fn parses_hello_with_missing_tools() {
        // git and runtime dir present, every other tool absent.
        let line = "hello\t2\tL3Vzci9iaW4vZ2l0\t\t\t\t\t\tL3J1bi91c2VyLzEwMDAvYWxhY3JpdHJlZQ==\n";
        let caps = parse_hello(line).unwrap();
        assert_eq!(caps.path("git"), Some("/usr/bin/git"));
        assert_eq!(caps.path("delta"), None);
        assert_eq!(caps.path("tuicr"), None);
        assert_eq!(caps.path("not-a-tool"), None);
        assert_eq!(caps.runtime_dir, "/run/user/1000/alacritree");
    }

    #[test]
    fn rejects_unknown_hello_version() {
        assert!(parse_hello("hello\t1\t\t\t\t\t\t\t\n").is_none());
        assert!(parse_hello("goodbye\t2\t\t\t\t\t\t\t\n").is_none());
        assert!(parse_hello("hello\t2\t\t\t\t\n").is_none());
    }

    #[test]
    fn hello_with_empty_trailing_field_still_parses() {
        let caps = parse_hello(HELLO_LINE).expect("empty fields are valid");
        assert_eq!(caps.path("git"), None);
        assert_eq!(caps.runtime_dir, "");
    }

    /// The script's probe list and the parser's field list are two spellings
    /// of one order; a tool added to one and not the other shifts every path
    /// after it onto the wrong name.
    #[test]
    fn the_hello_probes_every_tool_it_reports_in_order() {
        let probes: Vec<String> =
            HELLO_TOOLS.iter().map(|p| format!("command -v {p} || echo")).collect();
        assert!(HELPER_SCRIPT.contains(&format!("-lc '{}'", probes.join("; "))));
        assert!(HELPER_SCRIPT.contains(&format!("[ \"$i\" -le {} ]", HELLO_TOOLS.len())));
    }
```

In the ignored `helper_round_trips` test, replace `assert!(caps.git.is_some(), ...)` with:

```rust
        assert!(caps.path("git").is_some(), "test distros are expected to have git");
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p alacritree --locked wsl_helper::`
Expected: compile error, `HELLO_TOOLS` not found and no method `path` on `Capabilities`.

- [ ] **Step 3: Implement protocol 2**

Replace the protocol constant, `Capabilities` and `parse_hello` at the top of `wsl_helper.rs`:

```rust
/// Bumped only when the request/response framing changes incompatibly; a
/// client seeing any other version treats the helper as unusable and stays
/// on one-shot spawns.
pub const PROTOCOL_VERSION: &str = "2";

/// The programs the hello resolves, in the order its fields carry them.
/// `HELPER_SCRIPT` spells the same list.
pub const HELLO_TOOLS: [&str; 6] = ["git", "gh", "delta", "doppler", "herdr", "tuicr"];

/// Login-shell-resolved tool paths and the distro-side runtime dir, from
/// the helper's hello line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    /// One entry per [`HELLO_TOOLS`] name, `None` where the tool wasn't on
    /// the login shell's PATH at helper start.
    paths: Vec<Option<String>>,
    pub runtime_dir: String,
}

impl Capabilities {
    pub fn path(&self, program: &str) -> Option<&str> {
        let slot = HELLO_TOOLS.iter().position(|p| *p == program)?;
        self.paths.get(slot)?.as_deref()
    }
}
```

```rust
pub fn parse_hello(line: &str) -> Option<Capabilities> {
    // Strip only line terminators. trim_end() would also eat the tab before
    // a legitimately empty trailing field.
    let mut fields = line.trim_end_matches(['\r', '\n']).split('\t');
    if fields.next()? != "hello" || fields.next()? != PROTOCOL_VERSION {
        return None;
    }
    let mut decode = || -> Option<String> {
        let raw = B64.decode(fields.next()?).ok()?;
        Some(String::from_utf8_lossy(&raw).trim().to_string())
    };
    let paths = HELLO_TOOLS
        .iter()
        .map(|_| decode().map(|path| (!path.is_empty()).then_some(path)))
        .collect::<Option<Vec<_>>>()?;
    let runtime_dir = decode()?;
    Some(Capabilities { paths, runtime_dir })
}
```

In `HELPER_SCRIPT`, replace everything from the `caps=` line through the end of the hello `printf`, including its four `"$(b64 ...)"` lines, with:

```sh
caps=$("$s" -lc 'command -v git || echo; command -v gh || echo; command -v delta || echo; command -v doppler || echo; command -v herdr || echo; command -v tuicr || echo' 2>/dev/null)
rt=${XDG_RUNTIME_DIR:-/tmp}/alacritree
printf 'hello\t2'
i=1
while [ "$i" -le 6 ]; do
  printf '\t%s' "$(b64 "$(printf %s "$caps" | sed -n "${i}p")")"
  i=$((i + 1))
done
printf '\t%s\n' "$(b64 "$rt")"
```

Replace `capability_delta` and `capability_gh` with:

```rust
/// Where the helper's hello found `program`, one of [`HELLO_TOOLS`].
pub fn capability(distro: &str, program: &str) -> Option<String> {
    client(distro)?.capabilities()?.path(program).map(str::to_string)
}
```

Update the two callers:

```rust
// wsl.rs, inside discover_delta
    if let Some(path) = crate::wsl_helper::capability(distro, "delta") {
```

```rust
// pr_status.rs, inside query_gh's Wsl arm
            let gh = crate::wsl_helper::capability(&distro, "gh").unwrap_or_else(|| "gh".to_string());
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo nextest run -p alacritree --locked wsl_helper::`
Expected: PASS, including every `FakeHelper` test, which now parses the protocol 2 `HELLO_LINE`.

- [ ] **Step 5: Format, lint, commit**

```bash
devkit run task fmt && devkit run task clippy
git -C C:/Users/Lev/Git/github/alacritree-worktrees/feat-ux-configurable-diff-viewer add alacritree/src/wsl_helper.rs alacritree/src/wsl.rs alacritree/src/pr_status.rs
git -C C:/Users/Lev/Git/github/alacritree-worktrees/feat-ux-configurable-diff-viewer commit -m "refactor(wsl): resolve every tool in the helper hello" -m "The hello line carried git, delta and gh in fixed fields, so a new
external tool meant a new struct field and a new capability function.
It now carries one path per name in HELLO_TOOLS, and callers ask by
program name. The field count changes, so the protocol version goes
to 2.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 2: Tool registry, with the WSL delta lookup moved onto it

**Files:**
- Create: `alacritree/src/tools.rs`
- Modify: `alacritree/src/main.rs:62-66` (add `mod tools;` between `mod test_util;` and `mod upstream;`, keeping the `#[cfg(test)]` attribute on `test_util`)
- Modify: `alacritree/src/app/git_panel.rs:31-38, 52-53` (drop `wsl_delta_paths`, `pending_delta`), `:483-495` (`open_diff` WSL arm), `:522-559` (delete `wsl_delta_path`)
- Modify: `alacritree/src/wsl.rs:566-577` (delete `discover_delta`)

**Interfaces:**
- Consumes: `wsl_helper::HELLO_TOOLS`, `wsl_helper::capability(distro, program)` from Task 1; `wsl::probe_tools(distro, &[&str], &jobs::Blocking) -> Result<Vec<Option<String>>, String>`; `jobs::pool().spawn(Priority, FnOnce(&Blocking) -> T) -> Job<T>`, `Job::poll`, `Job::failed`.
- Produces:
  - `pub enum tools::Tool { Git, Gh, Delta, Doppler, Herdr, Tuicr }`, which derives `Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize`
  - `Tool::ALL: [Tool; 6]` in that order
  - `Tool::name(self) -> &'static str`
  - `pub struct tools::ToolPaths { pub native: String, pub wsl: Option<String> }` with `ToolPaths::named(Tool)`
  - `pub fn tools::program(tool: Tool) -> String`, the native side
  - `pub fn tools::wsl_resolved(tool: Tool, distro: &str, on_found: impl FnOnce() + Send + 'static) -> Option<String>`

- [ ] **Step 1: Write the failing tests**

Create `alacritree/src/tools.rs` holding only the tests for now:

```rust
#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::{Duration, Instant};

    use super::*;

    fn wait_until(mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done() {
            assert!(Instant::now() < deadline, "the lookup never landed");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn the_helper_hello_probes_the_registry_in_order() {
        assert_eq!(crate::wsl_helper::HELLO_TOOLS, Tool::ALL.map(Tool::name));
    }

    #[test]
    fn a_lookup_runs_once_per_distro_and_tool_and_keeps_its_hit() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (release, released) = mpsc::channel::<()>();
        let released = Mutex::new(released);
        let counted = calls.clone();
        let probe: Probe = Arc::new(move |_, _, _| {
            counted.fetch_add(1, Ordering::SeqCst);
            let _ = released.lock().unwrap().recv();
            Some("/home/lev/.cargo/bin/delta".to_string())
        });
        let mut lookups = Lookups::new(probe);

        assert_eq!(lookups.resolve("Ubuntu", Tool::Delta, Box::new(|| {})), None);
        assert_eq!(lookups.resolve("Ubuntu", Tool::Delta, Box::new(|| {})), None);
        release.send(()).unwrap();
        wait_until(|| lookups.cached("Ubuntu", Tool::Delta).is_some());

        assert_eq!(
            lookups.resolve("Ubuntu", Tool::Delta, Box::new(|| {})).as_deref(),
            Some("/home/lev/.cargo/bin/delta")
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(lookups.cached("kali-linux", Tool::Delta), None, "distros resolve apart");
    }

    #[test]
    fn a_miss_is_not_kept_so_the_next_call_looks_again() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let probe: Probe = Arc::new(move |_, _, _| {
            counted.fetch_add(1, Ordering::SeqCst);
            None
        });
        let mut lookups = Lookups::new(probe);

        assert_eq!(lookups.resolve("Ubuntu", Tool::Tuicr, Box::new(|| {})), None);
        wait_until(|| {
            lookups.adopt("Ubuntu", Tool::Tuicr);
            lookups.pending.is_empty()
        });
        assert_eq!(lookups.resolve("Ubuntu", Tool::Tuicr, Box::new(|| {})), None);
        wait_until(|| calls.load(Ordering::SeqCst) == 2);
    }

    #[test]
    fn a_landed_lookup_tells_its_caller() {
        let probe: Probe = Arc::new(|_, _, _| Some("/usr/bin/git".to_string()));
        let mut lookups = Lookups::new(probe);
        let (found, heard) = mpsc::channel();
        lookups.resolve("Ubuntu", Tool::Git, Box::new(move || found.send(()).unwrap()));
        heard.recv_timeout(Duration::from_secs(5)).expect("on_found runs when the probe lands");
    }
}
```

Add `mod tools;` to `main.rs`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p alacritree --locked tools::`
Expected: compile error, `Tool`, `Probe` and `Lookups` not found.

- [ ] **Step 3: Implement the registry**

Put this above the tests in `tools.rs`:

```rust
//! The external programs alacritree runs, and where each one lives.
//!
//! Each tool has a native path and an optional WSL path, because a Windows
//! path means nothing inside a distro.  A native path runs as written when it
//! differs from the tool's own name, and the name alone is found by the OS's
//! PATH search.  A set WSL path runs as written inside every distro; unset,
//! the name is found through the user's login shell, since `wsl.exe --exec`
//! sees only the default system PATH, which omits per-user install dirs like
//! `~/.cargo/bin`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, RwLock};

use crate::{jobs, wsl, wsl_helper};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub enum Tool {
    Git,
    Gh,
    Delta,
    Doppler,
    Herdr,
    Tuicr,
}

impl Tool {
    /// Declaration order, which `configure` indexes by.
    pub const ALL: [Tool; 6] =
        [Tool::Git, Tool::Gh, Tool::Delta, Tool::Doppler, Tool::Herdr, Tool::Tuicr];

    /// The program's name, which is also its default path.
    pub fn name(self) -> &'static str {
        match self {
            Tool::Git => "git",
            Tool::Gh => "gh",
            Tool::Delta => "delta",
            Tool::Doppler => "doppler",
            Tool::Herdr => "herdr",
            Tool::Tuicr => "tuicr",
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Where one tool lives on each side, as configured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPaths {
    /// The tool's own name, or a path that runs as written.
    pub native: String,
    /// A path that runs as written inside every distro, or `None` to find
    /// the tool by name there.
    pub wsl: Option<String>,
}

impl ToolPaths {
    pub fn named(tool: Tool) -> Self {
        Self { native: tool.name().to_string(), wsl: None }
    }
}

fn configured() -> &'static RwLock<[ToolPaths; 6]> {
    static PATHS: OnceLock<RwLock<[ToolPaths; 6]>> = OnceLock::new();
    PATHS.get_or_init(|| RwLock::new(Tool::ALL.map(ToolPaths::named)))
}

fn configured_paths(tool: Tool) -> ToolPaths {
    configured().read().unwrap_or_else(|e| e.into_inner())[tool as usize].clone()
}

/// The configured WSL path.  It is used as written and never looked up.
fn wsl_override(tool: Tool) -> Option<String> {
    configured_paths(tool).wsl
}

/// The program to spawn natively for `tool`: the configured path, which is
/// the bare name unless it was set.
pub fn program(tool: Tool) -> String {
    configured_paths(tool).native
}

/// The absolute path of `tool` inside `distro`, for the UI thread: the
/// configured path or a path an earlier lookup found, else `None` while a
/// background lookup runs.  Starts at most one lookup per distro and tool,
/// and `on_found` runs when it lands so the caller can repaint.  A miss is
/// never kept, so a tool installed mid-session is found by a later call.
pub fn wsl_resolved(
    tool: Tool,
    distro: &str,
    on_found: impl FnOnce() + Send + 'static,
) -> Option<String> {
    if let Some(path) = wsl_override(tool) {
        return Some(path);
    }
    lock(lookups()).resolve(distro, tool, Box::new(on_found))
}

type Probe = Arc<dyn Fn(&str, Tool, &jobs::Blocking) -> Option<String> + Send + Sync>;

fn lookups() -> &'static Mutex<Lookups> {
    static LOOKUPS: OnceLock<Mutex<Lookups>> = OnceLock::new();
    LOOKUPS.get_or_init(|| Mutex::new(Lookups::new(Arc::new(probe_distro))))
}

/// The helper's hello resolved every tool when the helper started.  A miss
/// there is not a kept miss: the live probe still sees a later install.
fn probe_distro(distro: &str, tool: Tool, blocking: &jobs::Blocking) -> Option<String> {
    wsl_helper::capability(distro, tool.name()).or_else(|| {
        wsl::probe_tools(distro, &[tool.name()], blocking).ok()?.into_iter().next().flatten()
    })
}

/// Paths found inside each distro, and the lookups still running.
struct Lookups {
    found: HashMap<(String, Tool), String>,
    pending: HashMap<(String, Tool), jobs::Job<Option<String>>>,
    probe: Probe,
}

impl Lookups {
    fn new(probe: Probe) -> Self {
        Self { found: HashMap::new(), pending: HashMap::new(), probe }
    }

    fn cached(&mut self, distro: &str, tool: Tool) -> Option<String> {
        self.adopt(distro, tool);
        self.found.get(&(distro.to_string(), tool)).cloned()
    }

    /// Bank a landed lookup.  A found-nothing landing and a panicked lookup
    /// both clear the pending entry, so neither wedges the tool out of ever
    /// being looked up again.
    fn adopt(&mut self, distro: &str, tool: Tool) {
        let key = (distro.to_string(), tool);
        match self.pending.get(&key).map(|job| (job.poll(), job.failed())) {
            Some((Some(Some(path)), _)) => {
                self.pending.remove(&key);
                self.found.insert(key, path);
            },
            Some((Some(None), _)) | Some((None, true)) => {
                self.pending.remove(&key);
            },
            _ => {},
        }
    }

    fn resolve(
        &mut self,
        distro: &str,
        tool: Tool,
        on_found: Box<dyn FnOnce() + Send>,
    ) -> Option<String> {
        if let Some(path) = self.cached(distro, tool) {
            return Some(path);
        }
        let key = (distro.to_string(), tool);
        if !self.pending.contains_key(&key) {
            let probe = self.probe.clone();
            let distro = distro.to_string();
            let job = jobs::pool().spawn(jobs::Priority::Background, move |blocking| {
                let found = probe(&distro, tool, blocking);
                on_found();
                found
            });
            self.pending.insert(key, job);
        }
        None
    }
}
```

If the compiler rejects the `static` because `Lookups` is not `Send`, the cause is `jobs::Job`. Fix it by storing `pending` as `HashMap<(String, Tool), Arc<Mutex<Option<Option<String>>>>>` filled by the job. Don't add `unsafe impl`.

In `app/git_panel.rs`, add `use crate::tools::{self, Tool};` after `use super::*;`. Delete the `wsl_delta_paths` and `pending_delta` fields, their doc comments and their initializers. Delete `wsl_delta_path`. Change the WSL arm of `open_diff`:

```rust
        let delta_override = self.config.delta_path.clone();
        let (program, args) = match wsl::classify(&workspace) {
            wsl::Location::Wsl { distro, .. } => {
                let repaint = ctx.clone();
                let delta = delta_override.or_else(|| {
                    tools::wsl_resolved(Tool::Delta, &distro, move || repaint.request_repaint())
                });
                match delta {
                    Some(delta) => build_wsl_diff_command_direct(&distro, &workspace, &req, &delta),
                    None => build_wsl_diff_command_login(&distro, &workspace, &req),
                }
            },
            wsl::Location::Windows(_) => {
                build_diff_command(delta_override.as_deref().unwrap_or("delta"), &req)
            },
        };
```

Delete `wsl::discover_delta` and its doc comment from `wsl.rs`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo nextest run -p alacritree --locked tools:: git_panel::`
Expected: PASS.

- [ ] **Step 5: Format, lint, commit**

```bash
devkit run task fmt && devkit run task clippy
git -C C:/Users/Lev/Git/github/alacritree-worktrees/feat-ux-configurable-diff-viewer add alacritree/src/tools.rs alacritree/src/main.rs alacritree/src/app/git_panel.rs alacritree/src/wsl.rs
git -C C:/Users/Lev/Git/github/alacritree-worktrees/feat-ux-configurable-diff-viewer commit -m "feat(tools): add a registry of external tools" -m "The git panel kept its own per-distro cache of delta's path, and the
next viewer tool would have needed a second one. The registry names
every external program alacritree runs and resolves a tool's path
inside a distro once, with the same keep-hits, retry-misses rule the
panel used. The panel's delta lookup moves onto it.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 3: Tool paths in config, with `[ui] delta_path` deprecated

**Files:**
- Modify: `alacritree/src/config.rs`:
  - `:63-67` and `:1436`: delete `Config::delta_path` and its default
  - `:523-586`: `IntegrationsConfig`, `ToolConfig`, `HerdrConfig::path`
  - `:2725-2800`: `RawIntegrations`, the tool tables, `RawHerdr::path`
  - `:2883-2886`: the `RawUi::delta_path` doc
  - `:3428-3431`: `into_config`
  - `:3862-3872`: tests
- Modify: `alacritree/src/tools.rs` to add `configure`
- Modify: `alacritree/src/main.rs:172`
- Modify: `alacritree/src/cli/doctor.rs:105` in `report`
- Modify: `alacritree/src/app/git_panel.rs`, the `open_diff` delta selection
- Regenerate: `schema/alacritree-config.json`, `alacritree/tests/stock-config.json`
- Modify: `docs/alacritree.md:522-523` and the `[integrations.herdr]` block at `:628`

**Interfaces:**
- Consumes: `tools::Tool`, `Tool::ALL`, `Tool::name`, `tools::program`, `tools::wsl_resolved` from Task 2.
- Produces:
  - `pub struct config::ToolConfig { pub path: String, pub wsl_path: Option<String> }`
  - `IntegrationsConfig { git, gh, doppler, delta, tuicr: ToolConfig, herdr: HerdrConfig }`
  - `HerdrConfig::path: String`, `HerdrConfig::wsl_path: Option<String>`
  - `IntegrationsConfig::paths(&self, Tool) -> tools::ToolPaths`
  - `IntegrationsConfig::tool_paths(&self) -> [tools::ToolPaths; 6]`
  - `pub fn tools::configure(paths: [ToolPaths; 6])`
  - `Config::delta_path` no longer exists.

- [ ] **Step 1: Write the failing tests**

In `config.rs` tests, delete `delta_path_parses_and_blank_is_none` and add the tests below. Add `use crate::tools::{Tool, ToolPaths};` to the test module if `super::*` does not bring them in.

```rust
    fn paths(native: &str, wsl: Option<&str>) -> ToolPaths {
        ToolPaths { native: native.to_string(), wsl: wsl.map(str::to_string) }
    }

    #[test]
    fn tool_paths_default_to_their_names_and_discovery_inside_wsl() {
        let config = config_from("");
        for tool in Tool::ALL {
            assert_eq!(config.integrations.paths(tool), ToolPaths::named(tool), "{tool:?}");
        }
    }

    #[test]
    fn a_tool_table_sets_each_side_and_blank_means_the_default() {
        let config = config_from(
            "[integrations.gh]\npath = \"C:/tools/gh.exe\"\nwsl_path = \"/opt/gh/bin/gh\"\n\
             [integrations.herdr]\nwsl_path = \"/home/lev/.local/bin/herdr\"\n\
             [integrations.tuicr]\npath = \"  \"\nwsl_path = \"  \"\n",
        );
        assert_eq!(
            config.integrations.paths(Tool::Gh),
            paths("C:/tools/gh.exe", Some("/opt/gh/bin/gh"))
        );
        assert_eq!(
            config.integrations.paths(Tool::Herdr),
            paths("herdr", Some("/home/lev/.local/bin/herdr"))
        );
        assert_eq!(config.integrations.paths(Tool::Tuicr), ToolPaths::named(Tool::Tuicr));
    }

    /// The deprecated key was one path used on both sides.  It keeps doing
    /// that for each side the new keys leave at its default.
    #[test]
    fn the_deprecated_ui_delta_path_fills_each_side_left_at_its_default() {
        let old = config_from("[ui]\ndelta_path = \"/opt/delta\"\n");
        assert_eq!(old.integrations.paths(Tool::Delta), paths("/opt/delta", Some("/opt/delta")));

        let native_set = config_from(
            "[ui]\ndelta_path = \"/opt/delta\"\n[integrations.delta]\npath = \"C:/delta.exe\"\n",
        );
        assert_eq!(
            native_set.integrations.paths(Tool::Delta),
            paths("C:/delta.exe", Some("/opt/delta"))
        );

        let both_set = config_from(
            "[ui]\ndelta_path = \"/opt/delta\"\n\
             [integrations.delta]\npath = \"C:/delta.exe\"\nwsl_path = \"/usr/local/bin/delta\"\n",
        );
        assert_eq!(
            both_set.integrations.paths(Tool::Delta),
            paths("C:/delta.exe", Some("/usr/local/bin/delta"))
        );

        let blank = config_from("[ui]\ndelta_path = \"  \"\n");
        assert_eq!(blank.integrations.paths(Tool::Delta), ToolPaths::named(Tool::Delta));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p alacritree --locked config::tests::tool_paths config::tests::a_tool_table config::tests::the_deprecated_ui`
Expected: compile error, no method `paths` on `IntegrationsConfig`.

- [ ] **Step 3: Implement the resolved types**

Replace the `IntegrationsConfig` definition in `config.rs`. `Default` is written by hand, because a derived one would give every tool an empty path.

```rust
/// `[integrations]`: how alacritree talks to the other tools it can see.  A
/// setting that exists only because of one tool belongs in that tool's table.
#[derive(Debug, Clone, serde::Serialize)]
pub struct IntegrationsConfig {
    pub git: ToolConfig,
    pub gh: ToolConfig,
    pub doppler: ToolConfig,
    pub herdr: HerdrConfig,
    pub delta: ToolConfig,
    pub tuicr: ToolConfig,
}

impl Default for IntegrationsConfig {
    fn default() -> Self {
        RawIntegrations::default().resolve(None)
    }
}

impl IntegrationsConfig {
    pub fn paths(&self, tool: Tool) -> ToolPaths {
        let (native, wsl) = match tool {
            Tool::Git => (&self.git.path, &self.git.wsl_path),
            Tool::Gh => (&self.gh.path, &self.gh.wsl_path),
            Tool::Delta => (&self.delta.path, &self.delta.wsl_path),
            Tool::Doppler => (&self.doppler.path, &self.doppler.wsl_path),
            Tool::Herdr => (&self.herdr.path, &self.herdr.wsl_path),
            Tool::Tuicr => (&self.tuicr.path, &self.tuicr.wsl_path),
        };
        ToolPaths { native: native.clone(), wsl: wsl.clone() }
    }

    /// Indexed like [`Tool::ALL`], the shape `tools::configure` takes.
    pub fn tool_paths(&self) -> [ToolPaths; 6] {
        Tool::ALL.map(|tool| self.paths(tool))
    }
}

/// `[integrations.<tool>]` for a tool with nothing to configure but where
/// it lives.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ToolConfig {
    /// The tool's own name, or a native path that runs as written.
    pub path: String,
    /// A path that runs as written inside every WSL distro, or `None` to
    /// find the tool by name there.
    pub wsl_path: Option<String>,
}
```

Add to `HerdrConfig`, as its first two fields:

```rust
    /// The native herdr binary, as `[integrations.herdr] path` names it.
    pub path: String,
    /// The herdr binary inside WSL, or `None` to find it by name there.
    pub wsl_path: Option<String>,
```

Add `use crate::tools::{Tool, ToolPaths};` to the `config.rs` imports.

- [ ] **Step 4: Implement the raw tables and resolution**

Replace `RawIntegrations` and add the tool tables beside it:

```rust
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(default)]
struct RawIntegrations {
    /// The git CLI, for the commands alacritree spawns.  Repository reads
    /// go through libgit2, and scripts run inside a WSL distro find git on
    /// that distro's own PATH.
    git: RawGit,
    /// The GitHub CLI behind PR badges and diff base branches.
    gh: RawGh,
    /// The Doppler CLI behind scope mirroring for new worktrees.
    doppler: RawDoppler,
    /// Agents running under a herdr server.
    herdr: RawHerdr,
    /// The pager the delta diff viewer runs.
    delta: RawDelta,
    /// The review TUI the tuicr diff viewer runs.
    tuicr: RawTuicr,
}

/// Declares an `[integrations.<tool>]` table whose only key is `path`.
macro_rules! raw_tool_table {
    ($raw:ident, $program:literal) => {
        #[derive(Debug, Deserialize, JsonSchema)]
        #[serde(default)]
        struct $raw {
            /// The program to run on Windows or natively.  Its own name is
            /// looked up on PATH; any other value runs as written.
            path: String,
            /// The program to run inside every WSL distro, as written.
            /// Empty finds it by name through the distro's login shell.
            wsl_path: String,
        }

        impl Default for $raw {
            fn default() -> Self {
                Self { path: $program.to_string(), wsl_path: String::new() }
            }
        }
    };
}

raw_tool_table!(RawGit, "git");
raw_tool_table!(RawGh, "gh");
raw_tool_table!(RawDoppler, "doppler");
raw_tool_table!(RawDelta, "delta");
raw_tool_table!(RawTuicr, "tuicr");

impl RawIntegrations {
    fn resolve(self, deprecated_delta_path: Option<String>) -> IntegrationsConfig {
        IntegrationsConfig {
            git: tool_config(self.git.path, self.git.wsl_path, Tool::Git),
            gh: tool_config(self.gh.path, self.gh.wsl_path, Tool::Gh),
            doppler: tool_config(self.doppler.path, self.doppler.wsl_path, Tool::Doppler),
            herdr: self.herdr.resolve(),
            delta: delta_config(self.delta.path, self.delta.wsl_path, deprecated_delta_path),
            tuicr: tool_config(self.tuicr.path, self.tuicr.wsl_path, Tool::Tuicr),
        }
    }
}

/// A blank path means the side's default, the same as leaving it out: the
/// tool's name natively, discovery inside WSL.
fn tool_config(path: String, wsl_path: String, tool: Tool) -> ToolConfig {
    ToolConfig {
        path: if path.trim().is_empty() { tool.name().to_string() } else { path },
        wsl_path: Some(wsl_path).filter(|p| !p.trim().is_empty()),
    }
}

/// `[ui] delta_path` was one path used on both sides.  It still fills each
/// side `[integrations.delta]` leaves at its default, because the raw config
/// structs accept unknown keys and dropping it would lose the override
/// silently.
fn delta_config(path: String, wsl_path: String, deprecated: Option<String>) -> ToolConfig {
    let mut delta = tool_config(path, wsl_path, Tool::Delta);
    let Some(old) = deprecated.filter(|old| !old.trim().is_empty()) else {
        return delta;
    };
    let native_default = delta.path == Tool::Delta.name();
    let wsl_default = delta.wsl_path.is_none();
    if native_default || wsl_default {
        log::warn!("[ui] delta_path is deprecated; set [integrations.delta] path and wsl_path");
    }
    if native_default {
        delta.path = old.clone();
    }
    if wsl_default {
        delta.wsl_path = Some(old);
    }
    delta
}
```

Add to `RawHerdr`, as its first two fields, with `path: "herdr".to_string()` and `wsl_path: String::new()` in its `Default`:

```rust
    /// The program to run on Windows or natively.  Its own name is looked
    /// up on PATH; any other value runs as written.
    path: String,
    /// The program to run inside every WSL distro, as written.  Empty finds
    /// it by name through the distro's login shell.
    wsl_path: String,
```

In `RawHerdr::resolve`, bind `let tool = tool_config(self.path, self.wsl_path, Tool::Herdr);` first, then set `path: tool.path, wsl_path: tool.wsl_path,`.

Replace the `RawUi::delta_path` doc comment:

```rust
    /// Deprecated location: `[integrations.delta] path` and `wsl_path`
    /// supersede this, and each wins on its own side once set.
    delta_path: Option<String>,
```

In `into_config`, delete `delta_path: ...` and change the integrations line to:

```rust
            integrations: self.integrations.resolve(self.ui.delta_path),
```

- [ ] **Step 5: Publish the configured paths at startup**

Add to `tools.rs`, after `configured()`:

```rust
/// Publish the configured paths of every tool, indexed like [`Tool::ALL`].
/// Runs once at startup, before anything spawns a tool.
pub fn configure(paths: [ToolPaths; 6]) {
    *configured().write().unwrap_or_else(|e| e.into_inner()) = paths;
}
```

In `main.rs`, after `wsl_helper::set_enabled(config.wsl_resident_helper);`:

```rust
    tools::configure(config.integrations.tool_paths());
```

In `cli/doctor.rs` `report`, directly after `let (config, _) = config::load(config_dir, overrides);`:

```rust
    tools::configure(config.integrations.tool_paths());
```

and add `tools` to the `use crate::{command_ext, jobs, state};` line.

In `open_diff`, the registry now carries the override. Replace the block from Task 2:

```rust
        let (program, args) = match wsl::classify(&workspace) {
            wsl::Location::Wsl { distro, .. } => {
                let repaint = ctx.clone();
                match tools::wsl_resolved(Tool::Delta, &distro, move || repaint.request_repaint()) {
                    Some(delta) => build_wsl_diff_command_direct(&distro, &workspace, &req, &delta),
                    None => build_wsl_diff_command_login(&distro, &workspace, &req),
                }
            },
            wsl::Location::Windows(_) => build_diff_command(&tools::program(Tool::Delta), &req),
        };
```

- [ ] **Step 6: Run the tests to verify they pass, then regenerate the fixtures**

Run: `cargo nextest run -p alacritree --locked config::`
Expected: PASS.

Run: `devkit run task test --env ALACRITREE_UPDATE_SCHEMA=1 --env ALACRITREE_UPDATE_STOCK=1`
Then: `devkit run task test`
Expected: PASS. Check the diffs:
- `git -C <worktree> diff -- schema/alacritree-config.json` shows `RawGit`, `RawGh`, `RawDoppler`, `RawDelta` and `RawTuicr` with `path` defaults equal to their names and `wsl_path` defaults of `""`, plus `herdr.path` defaulting to `"herdr"` and `herdr.wsl_path` to `""`.
- `git -C <worktree> diff -- alacritree/tests/stock-config.json` drops `delta_path` and adds the tool tables under `integrations`.
- `alacritree/tests/schema-defaults-allowlist.txt` is unchanged, and still lists `RawUi.delta_path`.

- [ ] **Step 7: Document the keys**

In `docs/alacritree.md`, delete the two `delta_path` lines from the `[ui]` block. Before `[integrations.herdr]`, add:

```toml
[integrations.git]          # one table per external program alacritree runs:
path     = "git"            # git, gh, doppler, herdr, delta and tuicr. path is
                            # the Windows or native program: the bare name is
                            # looked up on PATH, anything else runs as written
wsl_path = ""               # the program inside every WSL distro, run as
                            # written; empty finds it by name through the
                            # distro's login shell

[integrations.delta]
path     = "delta"          # the pager the delta diff viewer runs. These
wsl_path = ""               # supersede the deprecated [ui] delta_path, which
                            # still fills whichever of them is left at its
                            # default
```

Add `path = "herdr"` and `wsl_path = ""` as the first keys of the `[integrations.herdr]` block, with the comment `# the herdr binary on each side, set like the tables above`.

- [ ] **Step 8: Format, lint, commit**

```bash
devkit run task fmt && devkit run task clippy
git -C C:/Users/Lev/Git/github/alacritree-worktrees/feat-ux-configurable-diff-viewer add alacritree/src/config.rs alacritree/src/tools.rs alacritree/src/main.rs alacritree/src/cli/doctor.rs alacritree/src/app/git_panel.rs schema/alacritree-config.json alacritree/tests/stock-config.json docs/alacritree.md
git -C C:/Users/Lev/Git/github/alacritree-worktrees/feat-ux-configurable-diff-viewer commit -m "feat(config): set each external tool's path" -m "Only delta's path could be set, from [ui], and one value served both
Windows and WSL although a path means nothing on the other side. Each
registry tool now has an [integrations.<tool>] path for the native
side and a wsl_path that, left empty, finds the tool inside each
distro. [ui] delta_path stays readable because the raw structs accept
unknown keys; it fills each side the new keys leave at its default.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 4: Every tool spawn goes through the registry

**Files:**
- Modify: `alacritree/src/tools.rs` to add `wsl_in_job`
- Modify: `alacritree/src/pr_status.rs:575, 636, 667, 712`
- Modify: `alacritree/src/doppler.rs:119`
- Modify: `alacritree/src/worktree.rs:185, 191`. `735` and `750` are test code and stay.
- Modify: `alacritree/src/herdr/cli.rs:18-20` (`PROGRAM` becomes `program()`) plus every `side.command(PROGRAM, ...)` call, `alacritree/src/herdr/mod.rs:23`, `alacritree/src/multiplexer.rs:169, 250-268`, `alacritree/src/app/herdr_glue.rs:558`
- Modify: `alacritree/src/cli/doctor.rs:150-152, 186-203, 205-332`, tests `:785-829`

**Interfaces:**
- Consumes: `tools::program`, `Tool` (Task 2), `tools::configure` (Task 3), `wsl_helper::capability` (Task 1).
- Produces: `pub fn tools::wsl_program(tool: Tool) -> String`, `pub fn tools::wsl_in_job(tool: Tool, distro: &str, blocking: &jobs::Blocking) -> String`, `pub fn herdr::program(side: &Side) -> String`. `herdr::PROGRAM` no longer exists.

- [ ] **Step 1: Write the failing tests**

In `cli/doctor.rs` tests, rewrite the probe fixtures for six tools. Replace the bodies of these four tests:

```rust
    #[test]
    fn a_distro_reports_where_each_tool_resolved() {
        let found = probe(&[
            Some("/usr/bin/git"),
            Some("/home/lev/.local/bin/gh"),
            None,
            None,
            None,
            Some("/home/lev/.cargo/bin/tuicr"),
        ]);
        let detail = wsl_distro_check("Ubuntu", &found).detail;
        assert!(detail.contains("git /usr/bin/git"), "{detail:?}");
        assert!(detail.contains("gh /home/lev/.local/bin/gh"), "{detail:?}");
        assert!(detail.contains("tuicr /home/lev/.cargo/bin/tuicr"), "{detail:?}");
        assert!(detail.contains("no delta, doppler, herdr"), "{detail:?}");
    }

    #[test]
    fn a_distro_without_git_warns() {
        assert_eq!(wsl_distro_check("Ubuntu", &probe(&[None; 6])).status, Status::Warn);
        let git_only = probe(&[Some("/usr/bin/git"), None, None, None, None, None]);
        assert_eq!(wsl_distro_check("Ubuntu", &git_only).status, Status::Ok);
    }
```

```rust
    #[test]
    fn doppler_inside_a_distro_is_reported_as_unused() {
        let probes = vec![
            ("Ubuntu".to_string(), probe(&[None, None, None, Some("/usr/bin/doppler"), None, None])),
            ("kali-linux".to_string(), probe(&[None; 6])),
        ];
        let check = wsl_doppler_check(&probes).expect("a warning about the unused doppler");
        assert_eq!(check.status, Status::Warn);
        assert!(check.detail.contains("Ubuntu"), "{:?}", check.detail);
        assert!(!check.detail.contains("kali-linux"), "{:?}", check.detail);
    }

    #[test]
    fn no_distro_has_doppler_and_nothing_is_said() {
        let probes = vec![("Ubuntu".to_string(), probe(&[None; 6]))];
        assert!(wsl_doppler_check(&probes).is_none());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p alacritree --locked doctor::`
Expected: `a_distro_reports_where_each_tool_resolved` FAILS. The detail has no `tuicr`, because `WSL_TOOLS` still lists four tools and `tool_path` reads slots 0 to 3.

- [ ] **Step 3: Doctor probes the registry**

In `cli/doctor.rs`:
- Delete `const WSL_TOOLS` and its doc comment.
- Move the doppler sentence onto `probe_distros`: `/// Probes every registry tool. doppler is among them even though nothing runs it inside a distro, because [`wsl_doppler_check`] is the only place that says so.`
- In `probe_distros`: `let names = tools::Tool::ALL.map(tools::Tool::name);`, then `wsl::probe_tools(&name, &names, blocking)`. Clone `names` into the thread closure. It is a `[&'static str; 6]` and `Copy`.
- Change the `Probe` doc to "a path per entry of [`tools::Tool::ALL`]".
- `wsl_distro_check`:

```rust
    for tool in tools::Tool::ALL {
        match tool_path(found, tool) {
            Some(path) => present.push(format!("{} {path}", tool.name())),
            None => missing.push(tool.name()),
        }
    }
```

and `tool_path(found, tools::Tool::Git)` in its status line. `wsl_doppler_check` uses `tools::Tool::Doppler`.
- `tool_path`:

```rust
/// Where a probe put `tool`, by tool rather than by index, so
/// [`tools::Tool::ALL`] can be reordered without silently renaming
/// everyone's results.
fn tool_path(found: &[Option<String>], tool: tools::Tool) -> Option<&str> {
    let slot = tools::Tool::ALL.iter().position(|t| *t == tool)?;
    found.get(slot)?.as_deref()
}
```

- The native checks look up the configured path and keep the tool's name as the row name:

```rust
fn binary_checks() -> Vec<Check> {
    tools().iter().map(|tool| tool_check(tool, find(&configured_program(tool.program)))).collect()
}

/// A registry tool's configured path, which `locate` resolves as a path when
/// it is one; other programs are looked up by their own name.
fn configured_program(program: &str) -> String {
    tools::Tool::ALL
        .into_iter()
        .find(|tool| tool.name() == program)
        .map_or_else(|| program.to_string(), tools::program)
}
```

- `gh_auth_check`: `let gh = tools::program(tools::Tool::Gh); locate(&gh)?;`, then `command_ext::hidden(&gh)`.

- [ ] **Step 4: The other spawn sites**

Add to `tools.rs`, after `wsl_resolved`:

```rust
/// The program to name for `tool` inside a distro where a shell finds it:
/// the configured WSL path, else the bare name.
pub fn wsl_program(tool: Tool) -> String {
    wsl_override(tool).unwrap_or_else(|| tool.name().to_string())
}

/// The program to name for `tool` inside `distro` from a pool job: the
/// configured WSL path, a path a lookup already found, else what the
/// distro's resident helper resolved at start, else the bare name.  Off the
/// UI thread only, because reaching the helper can start it.
pub fn wsl_in_job(tool: Tool, distro: &str, _blocking: &jobs::Blocking) -> String {
    if let Some(path) = wsl_override(tool) {
        return path;
    }
    if let Some(path) = lock(lookups()).cached(distro, tool) {
        return path;
    }
    match wsl_helper::capability(distro, tool.name()) {
        Some(path) => {
            lock(lookups()).found.insert((distro.to_string(), tool), path.clone());
            path
        },
        None => tool.name().to_string(),
    }
}
```

`pr_status.rs`: add `use crate::tools::{self, Tool};`. The three native spawns become `command_ext::hidden(tools::program(Tool::Gh))`. In `query_gh`'s Wsl arm:

```rust
            let gh = tools::wsl_in_job(Tool::Gh, &distro, blocking);
```

Rewrite the comment above that arm so it names the registry instead of "the capability path from the helper's hello".

`doppler.rs`: `let mut cmd = command_ext::hidden(tools::program(Tool::Doppler));` with `use crate::tools::{self, Tool};`.

`worktree.rs` `git_command`: Windows arm `command_ext::hidden(tools::program(Tool::Git))`, WSL arm `cmd.arg(tools::wsl_program(Tool::Git)).arg("-C").arg(linux_path);`.

`herdr/cli.rs`: replace the `PROGRAM` constant:

```rust
/// The herdr binary every call on `side` runs, as `[integrations.herdr]`
/// names it for that side.  `Side::command` takes it as an argument so a
/// second multiplexer reaches its own through the same plumbing, and its
/// WSL form runs through a login shell that finds a bare name.
pub fn program(side: &Side) -> String {
    match side {
        Side::Native => tools::program(Tool::Herdr),
        Side::Wsl(_) => tools::wsl_program(Tool::Herdr),
    }
}
```

Change every `side.command(PROGRAM, ...)` call in that file to `side.command(&program(side), ...)`. In `herdr/mod.rs`, re-export `program` in place of `PROGRAM`. Change `multiplexer.rs:169` to `target.side.command(&herdr::program(&target.side), &borrowed)`, `app/herdr_glue.rs:558` to `key.side.command(&herdr::program(&key.side), ...)`, and each of the three `multiplexer.rs` tests to pass `&herdr::program(&side)` for the side it builds.

After editing, confirm nothing is left:

Run: `rg -n 'PROGRAM\b|capability_(delta|gh)|discover_delta|hidden\("(gh|doppler)"\)' alacritree/src`
Expected: no matches.

Run: `rg -n 'hidden\("git"\)|\.arg\("git"\)' alacritree/src`
Expected: matches only inside `#[cfg(test)]` code, at `projects.rs:509` and `worktree.rs:735, 750`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `devkit run task test`
Expected: PASS.

- [ ] **Step 6: Format, lint, commit**

```bash
devkit run task fmt && devkit run task clippy
git -C C:/Users/Lev/Git/github/alacritree-worktrees/feat-ux-configurable-diff-viewer add alacritree/src
git -C C:/Users/Lev/Git/github/alacritree-worktrees/feat-ux-configurable-diff-viewer commit -m "refactor(tools): spawn every tool via the registry" -m "gh, doppler, herdr and git were spawned by literal name in five
modules, and doctor probed distros from its own list. Each spawn now
takes its program from the registry, so a configured path applies
everywhere the tool runs, and doctor reports every registry tool
inside each distro.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 5: `diff_viewer` module, with rows opened through `plan`

The diff types and argv builders move out of the git panel into a pure module. That module also learns viewers, section targets and placeholder templates. With the delta viewer hardcoded, the pane still behaves exactly as before.

**Files:**
- Create: `alacritree/src/diff_viewer.rs`
- Modify: `alacritree/src/main.rs` to add `mod diff_viewer;` between `mod decoration_sprites;` and `mod digest;`
- Modify: `alacritree/src/app/git_panel.rs`:
  - delete `:1090-1118`, `:1147-1242` and the moved tests `:1320-1416`
  - change `open_diff` `:454-520`, `apply_git_sidebar_requests` `:445-452` and `apply_git_sidebar_nav` `:229-235`

**Interfaces:**
- Consumes: `tools::Tool`.
- Produces, all `pub` in `crate::diff_viewer`:
  - `DiffSource { Staged, Worktree, Untracked, Branch { base: String } }`
  - `DiffRequest { file: String, source: DiffSource }`
  - `diff_key(&DiffRequest) -> String`
  - `diff_args(&DiffRequest) -> Vec<String>`
  - `Section { Staged, Unstaged, Branch { base: String } }`, with `Section::label(&self) -> String`
  - `Target { Row(DiffRequest), Section(Section) }`, with `Target::key(&self) -> String`
  - `Program { Tool(Tool), Custom { path: String, wsl_path: Option<String> } }`
  - `Templates { staged, unstaged, untracked, branch, staged_scope, unstaged_scope, branch_scope: Vec<String> }`
  - `Viewer { Pager { pager: Program, args: Vec<String> }, Direct { program: Program, templates: Templates } }`, with `Viewer::delta()`, `Viewer::tuicr()`, `Viewer::program(&self) -> &Program`
  - `Launch { Pager { pager: Program, pager_args: Vec<String>, git_args: Vec<String> }, Direct { program: Program, args: Vec<String> } }`
  - `opens(&Viewer, &Target) -> bool`
  - `plan(&Viewer, &Target) -> Option<Launch>`
  - `pager_command(pager: &str, args: &[String]) -> String`
  - `native_pager_command(git: &str, pager: &str, git_args: &[String]) -> (String, Vec<String>)`
  - `wsl_pager_command(distro: &str, workspace: &Path, git: &str, pager: &str, git_args: &[String]) -> (String, Vec<String>)`
  - `wsl_pager_command_login(same args) -> (String, Vec<String>)`
  - `wsl_direct_command(distro: &str, workspace: &Path, program: &str, args: &[String]) -> (String, Vec<String>)`
  - `wsl_direct_command_login(same args) -> (String, Vec<String>)`

- [ ] **Step 1: Write the failing tests**

Create `alacritree/src/diff_viewer.rs` holding only the tests:

```rust
#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn row(file: &str, source: DiffSource) -> Target {
        Target::Row(DiffRequest { file: file.to_string(), source })
    }

    fn branch() -> DiffSource {
        DiffSource::Branch { base: "refs/remotes/origin/main".to_string() }
    }

    fn direct_args(viewer: &Viewer, target: &Target) -> Option<Vec<String>> {
        match plan(viewer, target)? {
            Launch::Direct { args, .. } => Some(args),
            Launch::Pager { .. } => panic!("a direct viewer planned a pager launch"),
        }
    }

    fn git_args(target: &Target) -> Vec<String> {
        match plan(&Viewer::delta(), target).expect("delta opens everything") {
            Launch::Pager { git_args, .. } => git_args,
            Launch::Direct { .. } => panic!("delta planned a direct launch"),
        }
    }

    #[test]
    fn diff_args_per_source() {
        let req = |source| DiffRequest { file: "a.rs".to_string(), source };
        assert_eq!(diff_args(&req(DiffSource::Staged)), ["diff", "--cached", "--", "a.rs"]);
        assert_eq!(diff_args(&req(DiffSource::Worktree)), ["diff", "--", "a.rs"]);
        assert_eq!(diff_args(&req(DiffSource::Untracked)), [
            "diff",
            "--no-index",
            "--",
            "/dev/null",
            "a.rs"
        ]);
        let base = DiffSource::Branch { base: "main".to_string() };
        assert_eq!(diff_args(&req(base)), ["diff", "main...", "--", "a.rs"]);
    }

    #[test]
    fn row_and_section_keys_never_collide() {
        assert_eq!(row("a.rs", DiffSource::Staged).key(), "staged:a.rs");
        assert_eq!(row("a.rs", branch()).key(), "branch:a.rs");
        assert_eq!(Target::Section(Section::Staged).key(), "section:staged");
        assert_eq!(Target::Section(Section::Unstaged).key(), "section:unstaged");
        assert_eq!(Target::Section(Section::Branch { base: "main".into() }).key(), "section:branch");
    }

    #[test]
    fn delta_pipes_each_target_through_the_pager() {
        let launch = plan(&Viewer::delta(), &row("a.rs", DiffSource::Staged)).unwrap();
        assert_eq!(launch, Launch::Pager {
            pager: Program::Tool(Tool::Delta),
            pager_args: vec!["--paging=always".to_string()],
            git_args: vec!["diff".into(), "--cached".into(), "--".into(), "a.rs".into()],
        });
        assert_eq!(git_args(&Target::Section(Section::Staged)), ["diff", "--cached"]);
        assert_eq!(git_args(&Target::Section(Section::Unstaged)), ["diff"]);
        assert_eq!(git_args(&Target::Section(Section::Branch { base: "main".into() })), [
            "diff", "main..."
        ]);
    }

    #[test]
    fn tuicr_reviews_rows_by_path_and_sections_by_scope() {
        let tuicr = Viewer::tuicr();
        for source in [DiffSource::Staged, DiffSource::Worktree, DiffSource::Untracked] {
            assert_eq!(direct_args(&tuicr, &row("a.rs", source)).unwrap(), ["-w", "-p", "a.rs"]);
        }
        assert_eq!(direct_args(&tuicr, &row("a.rs", branch())).unwrap(), [
            "-r",
            "refs/remotes/origin/main...HEAD",
            "-p",
            "a.rs"
        ]);
        assert_eq!(direct_args(&tuicr, &Target::Section(Section::Staged)).unwrap(), ["-w"]);
        assert_eq!(direct_args(&tuicr, &Target::Section(Section::Unstaged)).unwrap(), ["-w"]);
        let changes = Target::Section(Section::Branch { base: "main".into() });
        assert_eq!(direct_args(&tuicr, &changes).unwrap(), ["-r", "main...HEAD"]);
        assert_eq!(tuicr.program(), &Program::Tool(Tool::Tuicr));
    }

    #[test]
    fn a_file_name_stays_one_argument() {
        let args = direct_args(&Viewer::tuicr(), &row("dir/my {base} 'x'.rs", DiffSource::Staged));
        assert_eq!(args.unwrap(), ["-w", "-p", "dir/my {base} 'x'.rs"]);
    }

    #[test]
    fn an_empty_template_or_a_missing_placeholder_value_opens_nothing() {
        let viewer = Viewer::Direct {
            program: Program::Custom { path: "difft".to_string(), wsl_path: None },
            templates: Templates {
                staged: vec!["--base".to_string(), "{base}".to_string()],
                ..Templates::default()
            },
        };
        let staged_row = row("a.rs", DiffSource::Staged);
        assert!(!opens(&viewer, &staged_row));
        assert!(plan(&viewer, &staged_row).is_none(), "a staged row has no base");
        let unstaged_row = row("a.rs", DiffSource::Worktree);
        assert!(!opens(&viewer, &unstaged_row));
        assert!(plan(&viewer, &unstaged_row).is_none(), "the unstaged template is empty");
        assert!(opens(&Viewer::delta(), &Target::Section(Section::Unstaged)));
    }

    #[test]
    fn a_native_pager_launch_runs_git_with_the_pager_wired_in() {
        let pager = pager_command(r"C:\tools\delta.exe", &["--paging=always".to_string()]);
        let git_args = ["diff".to_string(), "--".to_string(), "a.rs".to_string()];
        let (program, args) = native_pager_command("git", &pager, &git_args);
        assert_eq!(program, "git");
        assert_eq!(args, ["-c", r"core.pager=C:\tools\delta.exe --paging=always", "diff", "--", "a.rs"]);
    }

    const WORKSPACE: &str = r"\\wsl.localhost\kali-linux\home\lev\proj";

    #[test]
    fn a_wsl_pager_launch_passes_git_pager_and_diff_as_positional_parameters() {
        let git_args = ["diff".to_string(), "--cached".to_string()];
        let (program, args) =
            wsl_pager_command("kali-linux", Path::new(WORKSPACE), "git", "/bin/delta --paging=always", &git_args);
        assert_eq!(program, "wsl.exe");
        assert_eq!(args[..7], ["-d", "kali-linux", "--cd", WORKSPACE, "--exec", "sh", "-c"]);
        assert_eq!(args[7], r#"export LESS="${LESS-R}"; g=$1; p=$2; shift 2; exec "$g" -c "core.pager=$p" "$@""#);
        assert_eq!(args[8..], ["sh", "git", "/bin/delta --paging=always", "diff", "--cached"]);
    }

    #[test]
    fn a_login_wsl_pager_launch_exports_less_after_the_profile() {
        let (_, args) = wsl_pager_command_login(
            "kali-linux",
            Path::new(WORKSPACE),
            "git",
            "delta --paging=always",
            &["diff".to_string()],
        );
        let script = &args[7];
        assert!(script.contains("getent passwd"), "resolves the login shell: {script}");
        assert!(
            script.contains(r#"-lc 'export LESS="${LESS-R}"; g=$1; p=$2; shift 2; exec "$g" -c "core.pager=$p" "$@"' "$s" "$@""#),
            "a LESS set by the profile still wins: {script}"
        );
        assert_eq!(args[8..], ["sh", "git", "delta --paging=always", "diff"]);
    }

    #[test]
    fn a_wsl_direct_launch_execs_the_resolved_program() {
        let (program, args) = wsl_direct_command(
            "kali-linux",
            Path::new(WORKSPACE),
            "/home/lev/.cargo/bin/tuicr",
            &["-w".to_string(), "-p".to_string(), "a b.rs".to_string()],
        );
        assert_eq!(program, "wsl.exe");
        assert_eq!(args, [
            "-d",
            "kali-linux",
            "--cd",
            WORKSPACE,
            "--exec",
            "/home/lev/.cargo/bin/tuicr",
            "-w",
            "-p",
            "a b.rs"
        ]);
    }

    #[test]
    fn a_login_wsl_direct_launch_passes_the_program_as_a_parameter() {
        let (_, args) =
            wsl_direct_command_login("kali-linux", Path::new(WORKSPACE), "tuicr", &["-w".to_string()]);
        assert_eq!(args[..7], ["-d", "kali-linux", "--cd", WORKSPACE, "--exec", "sh", "-c"]);
        assert!(args[7].contains("getent passwd"));
        assert!(args[7].ends_with(r#"exec "$s" -lc 'exec "$@"' "$s" "$@""#), "{}", args[7]);
        assert_eq!(args[8..], ["sh", "tuicr", "-w"]);
    }
}
```

Add `mod diff_viewer;` to `main.rs`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p alacritree --locked diff_viewer::`
Expected: compile error, `DiffSource`, `Target`, `Viewer` and `plan` not found.

- [ ] **Step 3: Implement the module**

Put this above the tests. `DiffSource`, `DiffRequest`, `diff_key` and `diff_args` move from `git_panel.rs` with their comments, their visibility changes to `pub`, and they gain the derives shown.

```rust
//! What the git panel's diff pane opens and the command that opens it.
//!
//! A viewer either lets git render the diff and pipe it through a pager, or
//! renders the diff itself from an argv template.  The built-in viewers are
//! values of the same type a custom one resolves to, so every viewer takes
//! one path from a click to a spawn.

use std::path::Path;

use crate::tools::Tool;

/// Which `git diff` flavor a git panel row opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffSource {
    Staged,
    Worktree,
    Untracked,
    /// Triple-dot diff against this base ref (merge-base, matching the
    /// `Changes vs <branch>` sidebar section).
    Branch {
        base: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRequest {
    pub file: String,
    pub source: DiffSource,
}

/// A whole git panel section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Section {
    Staged,
    Unstaged,
    Branch { base: String },
}

impl Section {
    /// What the pane's tab calls the section.
    pub fn label(&self) -> String {
        match self {
            Section::Staged => "staged changes".to_string(),
            Section::Unstaged => "unstaged changes".to_string(),
            Section::Branch { base } => format!("changes vs {base}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Row(DiffRequest),
    Section(Section),
}

impl Target {
    /// Stable identity of the pane this target opens, matched against the
    /// open diff session's `SessionKind::Diff { key }` to highlight what
    /// opened it and to close the pane when it is chosen again.
    pub fn key(&self) -> String {
        match self {
            Target::Row(req) => diff_key(req),
            Target::Section(Section::Staged) => "section:staged".to_string(),
            Target::Section(Section::Unstaged) => "section:unstaged".to_string(),
            Target::Section(Section::Branch { .. }) => "section:branch".to_string(),
        }
    }

    fn file(&self) -> Option<&str> {
        match self {
            Target::Row(req) => Some(&req.file),
            Target::Section(_) => None,
        }
    }

    fn base(&self) -> Option<&str> {
        match self {
            Target::Row(DiffRequest { source: DiffSource::Branch { base }, .. })
            | Target::Section(Section::Branch { base }) => Some(base),
            _ => None,
        }
    }
}

/// A program the registry resolves, or one a custom viewer names.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum Program {
    Tool(Tool),
    /// `path` runs natively as written.  Inside WSL, `wsl_path` runs as
    /// written, and without one `path` goes through the distro's login
    /// shell, which finds a bare name.
    Custom { path: String, wsl_path: Option<String> },
}

/// One argv template per row kind and per section.  `{file}` is the row's
/// path and `{base}` the branch the `Changes vs` section diffs against.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Templates {
    pub staged: Vec<String>,
    pub unstaged: Vec<String>,
    pub untracked: Vec<String>,
    pub branch: Vec<String>,
    pub staged_scope: Vec<String>,
    pub unstaged_scope: Vec<String>,
    pub branch_scope: Vec<String>,
}

impl Templates {
    /// tuicr cannot split staged from unstaged, and its working-tree diff
    /// includes untracked files, so every uncommitted target opens `-w`.
    /// Its revision parser reads `A...B` as a merge-base range, matching the
    /// panel's triple-dot branch diff.
    fn tuicr() -> Self {
        let words = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        let uncommitted_row = words(&["-w", "-p", "{file}"]);
        Self {
            staged: uncommitted_row.clone(),
            unstaged: uncommitted_row.clone(),
            untracked: uncommitted_row,
            branch: words(&["-r", "{base}...HEAD", "-p", "{file}"]),
            staged_scope: words(&["-w"]),
            unstaged_scope: words(&["-w"]),
            branch_scope: words(&["-r", "{base}...HEAD"]),
        }
    }

    fn for_target(&self, target: &Target) -> &[String] {
        match target {
            Target::Row(req) => match req.source {
                DiffSource::Staged => &self.staged,
                DiffSource::Worktree => &self.unstaged,
                DiffSource::Untracked => &self.untracked,
                DiffSource::Branch { .. } => &self.branch,
            },
            Target::Section(Section::Staged) => &self.staged_scope,
            Target::Section(Section::Unstaged) => &self.unstaged_scope,
            Target::Section(Section::Branch { .. }) => &self.branch_scope,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum Viewer {
    /// git renders the diff and runs `pager` with `args` as its `core.pager`.
    Pager { pager: Program, args: Vec<String> },
    /// `program` renders the diff itself from its template's argv.
    Direct { program: Program, templates: Templates },
}

impl Viewer {
    pub fn delta() -> Self {
        Viewer::Pager {
            pager: Program::Tool(Tool::Delta),
            args: vec!["--paging=always".to_string()],
        }
    }

    pub fn tuicr() -> Self {
        Viewer::Direct { program: Program::Tool(Tool::Tuicr), templates: Templates::tuicr() }
    }

    pub fn program(&self) -> &Program {
        match self {
            Viewer::Pager { pager, .. } => pager,
            Viewer::Direct { program, .. } => program,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    Pager { pager: Program, pager_args: Vec<String>, git_args: Vec<String> },
    Direct { program: Program, args: Vec<String> },
}

/// Whether `viewer` can open `target`: always for a pager, and for a direct
/// viewer when the target's template is non-empty and every placeholder it
/// names has a value.
pub fn opens(viewer: &Viewer, target: &Target) -> bool {
    match viewer {
        Viewer::Pager { .. } => true,
        Viewer::Direct { templates, .. } => {
            let template = templates.for_target(target);
            !template.is_empty()
                && template.iter().all(|arg| {
                    (!arg.contains("{file}") || target.file().is_some())
                        && (!arg.contains("{base}") || target.base().is_some())
                })
        },
    }
}

/// How `viewer` opens `target`, or `None` when [`opens`] says it cannot.
pub fn plan(viewer: &Viewer, target: &Target) -> Option<Launch> {
    if !opens(viewer, target) {
        return None;
    }
    Some(match viewer {
        Viewer::Pager { pager, args } => Launch::Pager {
            pager: pager.clone(),
            pager_args: args.clone(),
            git_args: target_git_args(target),
        },
        Viewer::Direct { program, templates } => Launch::Direct {
            program: program.clone(),
            args: templates.for_target(target).iter().map(|arg| substitute(arg, target)).collect(),
        },
    })
}

fn target_git_args(target: &Target) -> Vec<String> {
    match target {
        Target::Row(req) => diff_args(req),
        Target::Section(Section::Staged) => vec!["diff".to_string(), "--cached".to_string()],
        Target::Section(Section::Unstaged) => vec!["diff".to_string()],
        Target::Section(Section::Branch { base }) => vec!["diff".to_string(), format!("{base}...")],
    }
}

/// Replaces `{file}` and `{base}` in one left-to-right pass inside a single
/// argument, so a file name never reaches a shell parser and text a
/// placeholder inserts is never scanned again.  `opens` has already checked
/// that every placeholder has a value.
fn substitute(arg: &str, target: &Target) -> String {
    let mut out = String::with_capacity(arg.len());
    let mut rest = arg;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let tail = &rest[open..];
        if let Some(after) = tail.strip_prefix("{file}") {
            out.push_str(target.file().unwrap_or_default());
            rest = after;
        } else if let Some(after) = tail.strip_prefix("{base}") {
            out.push_str(target.base().unwrap_or_default());
            rest = after;
        } else {
            out.push('{');
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}
```

Move `diff_key` and `diff_args` over unchanged, apart from `pub`. Then add the command builders, which replace `build_diff_command`, `build_wsl_diff_command_direct` and `build_wsl_diff_command_login`:

```rust
/// `pager` with its arguments, as one `core.pager` value.
pub fn pager_command(pager: &str, args: &[String]) -> String {
    std::iter::once(pager).chain(args.iter().map(String::as_str)).collect::<Vec<_>>().join(" ")
}

/// git with the pager wired in as its `core.pager`, so git drives the pipe
/// and no shell sits between them on any platform.  Paths and branches stay
/// in argv, so no file name is shell-parsed.
pub fn native_pager_command(git: &str, pager: &str, git_args: &[String]) -> (String, Vec<String>) {
    let mut args = vec!["-c".to_string(), format!("core.pager={pager}")];
    args.extend(git_args.iter().cloned());
    (git.to_string(), args)
}

/// Finds the user's login shell, falling back to `$SHELL`, into `$s`.
const LOGIN_SHELL: &str =
    r#"s=$(getent passwd "$(id -un)" 2>/dev/null | cut -d: -f7); [ -x "$s" ] || s=${SHELL:-/bin/sh}"#;

/// Runs git with the pager wired in, taking git as `$1`, the pager as `$2`
/// and the diff arguments after them, so no file name is shell-parsed.
///
/// The `LESS=R` the diff pane puts in the child's environment stays on the
/// Windows side of the wsl.exe boundary (only `WSLENV`-listed variables
/// cross), so git in the distro would hand its pager `LESS=FRX` and `F`
/// (quit-if-one-screen) would reap short diffs on open.  The script exports
/// `LESS` itself where git runs.
const PAGER_SCRIPT: &str =
    r#"export LESS="${LESS-R}"; g=$1; p=$2; shift 2; exec "$g" -c "core.pager=$p" "$@""#;

/// A pager launch inside the distro when the pager's path is known: a plain
/// `sh` finds it without sourcing a login profile.
pub fn wsl_pager_command(
    distro: &str,
    workspace: &Path,
    git: &str,
    pager: &str,
    git_args: &[String],
) -> (String, Vec<String>) {
    let positional = [git.to_string(), pager.to_string()].into_iter().chain(git_args.iter().cloned());
    wsl_sh(distro, workspace, PAGER_SCRIPT.to_string(), positional)
}

/// A pager launch before the pager's path is known: re-exec through the
/// login shell so the pager resolves from the user's real PATH.  `LESS` is
/// exported after the profile is sourced, so a profile-set `LESS` wins,
/// mirroring the `[env]` precedence on the Windows side.
pub fn wsl_pager_command_login(
    distro: &str,
    workspace: &Path,
    git: &str,
    pager: &str,
    git_args: &[String],
) -> (String, Vec<String>) {
    let script = format!(r#"{LOGIN_SHELL}; exec "$s" -lc '{PAGER_SCRIPT}' "$s" "$@""#);
    let positional = [git.to_string(), pager.to_string()].into_iter().chain(git_args.iter().cloned());
    wsl_sh(distro, workspace, script, positional)
}

/// A direct launch whose program path is known: `--exec` runs it with no
/// shell at all.
pub fn wsl_direct_command(
    distro: &str,
    workspace: &Path,
    program: &str,
    args: &[String],
) -> (String, Vec<String>) {
    let mut argv = vec![
        "-d".to_string(),
        distro.to_string(),
        "--cd".to_string(),
        workspace.to_string_lossy().into_owned(),
        "--exec".to_string(),
        program.to_string(),
    ];
    argv.extend(args.iter().cloned());
    ("wsl.exe".to_string(), argv)
}

/// A direct launch before the program's path is known: the login shell
/// resolves the program, which arrives as a parameter rather than as script
/// text.
pub fn wsl_direct_command_login(
    distro: &str,
    workspace: &Path,
    program: &str,
    args: &[String],
) -> (String, Vec<String>) {
    let script = format!(r#"{LOGIN_SHELL}; exec "$s" -lc 'exec "$@"' "$s" "$@""#);
    let positional = std::iter::once(program.to_string()).chain(args.iter().cloned());
    wsl_sh(distro, workspace, script, positional)
}

fn wsl_sh(
    distro: &str,
    workspace: &Path,
    script: String,
    positional: impl IntoIterator<Item = String>,
) -> (String, Vec<String>) {
    let mut args = vec![
        "-d".to_string(),
        distro.to_string(),
        "--cd".to_string(),
        workspace.to_string_lossy().into_owned(),
        "--exec".to_string(),
        "sh".to_string(),
        "-c".to_string(),
        script,
        "sh".to_string(),
    ];
    args.extend(positional);
    ("wsl.exe".to_string(), args)
}
```

- [ ] **Step 4: The git panel opens targets**

In `app/git_panel.rs`:
- Delete the moved items and the tests for them: `diff_args_*`, `diff_command_*`, `wsl_diff_*`, and the `req` helper.
- Add `use crate::diff_viewer::{self, DiffRequest, DiffSource, Launch, Program, Target, Viewer, diff_key};`.
- `apply_git_sidebar_nav` Enter: `self.open_diff(ctx, Target::Row(req));`
- `apply_git_sidebar_requests`: `self.open_diff(ctx, Target::Row(request));`
- Replace `open_diff`:

```rust
    /// Choosing a row or section either opens, replaces, or closes the
    /// workspace's single diff pane:
    /// - the target matches the open pane: close it
    /// - the viewer cannot open the target: leave everything as it is
    /// - otherwise: drop any other pane and open this one
    /// Dropping the old `Session` runs `Drop`, which sends `Msg::Shutdown` to
    /// the event loop and exits the viewer cleanly.
    fn open_diff(&mut self, ctx: &Context, target: Target) {
        let Some(workspace) = self.current_workspace.clone() else {
            return;
        };
        let new_key = target.key();
        let existing = self
            .sessions
            .iter()
            .find(|s| {
                s.working_directory.as_deref() == Some(&workspace)
                    && matches!(&s.kind, SessionKind::Diff { .. })
            })
            .map(|s| (s.id, matches!(&s.kind, SessionKind::Diff { key } if key == &new_key)));
        if let Some((id, true)) = existing {
            // Routing through close_session applies the same
            // sibling-promotion and fallback navigation as any other
            // close, so toggling off the diff pane never strands the
            // workspace on an empty view.
            self.close_session(ctx, id);
            return;
        }
        let Some(launch) = diff_viewer::plan(&Viewer::delta(), &target) else {
            return;
        };
        if let Some((id, _)) = existing {
            self.sessions.retain(|s| s.id != id);
        }

        let (program, args) = match wsl::classify(&workspace) {
            wsl::Location::Wsl { distro, .. } => wsl_diff_command(ctx, &distro, &workspace, launch),
            wsl::Location::Windows(_) => native_diff_command(launch),
        };
        let title = match &target {
            Target::Row(req) => format!(
                "diff: {}",
                path_style::render(&req.file, self.config.ui.path_style.diff_title, None)
            ),
            Target::Section(section) => format!("diff: {}", section.label()),
        };
        let (size, cell_size) = self.next_spawn_geometry();
        let (session, request) = Session::pending_command(
            ctx.clone(),
            &self.config,
            Some(workspace.clone()),
            size,
            cell_size,
            program,
            args,
            title,
            SessionKind::Diff { key: new_key },
        );
        match self.open_session(session, request) {
            Ok(id) => {
                self.active_session.insert(Some(workspace), id);
            },
            Err(e) => {
                self.modals.error_dialog = Some(format!("failed to open diff: {e}"));
            },
        }
    }
```

Add these free functions after the `impl AlacritreeApp` block that holds `active_diff_key`:

```rust
fn native_program(program: &Program) -> String {
    match program {
        Program::Tool(tool) => tools::program(*tool),
        Program::Custom { path, .. } => path.clone(),
    }
}

fn native_diff_command(launch: Launch) -> (String, Vec<String>) {
    match launch {
        Launch::Pager { pager, pager_args, git_args } => diff_viewer::native_pager_command(
            &tools::program(Tool::Git),
            &diff_viewer::pager_command(&native_program(&pager), &pager_args),
            &git_args,
        ),
        Launch::Direct { program, args } => (native_program(&program), args),
    }
}

/// The program's path inside `distro`, or `None` when the login shell has
/// to find it: a registry tool whose first lookup is still running, or a
/// custom viewer with no WSL path.
fn wsl_program(ctx: &Context, distro: &str, program: &Program) -> Option<String> {
    match program {
        Program::Tool(tool) => {
            let repaint = ctx.clone();
            tools::wsl_resolved(*tool, distro, move || repaint.request_repaint())
        },
        Program::Custom { wsl_path, .. } => wsl_path.clone(),
    }
}

/// What the login shell looks up when `wsl_program` has no path: a registry
/// tool's own name, or a custom viewer's native value.
fn program_name(program: &Program) -> &str {
    match program {
        Program::Tool(tool) => tool.name(),
        Program::Custom { path, .. } => path,
    }
}

fn wsl_diff_command(
    ctx: &Context,
    distro: &str,
    workspace: &Path,
    launch: Launch,
) -> (String, Vec<String>) {
    let git = tools::program(Tool::Git);
    match launch {
        Launch::Pager { pager, pager_args, git_args } => match wsl_program(ctx, distro, &pager) {
            Some(path) => {
                let pager = diff_viewer::pager_command(&path, &pager_args);
                diff_viewer::wsl_pager_command(distro, workspace, &git, &pager, &git_args)
            },
            None => {
                let pager = diff_viewer::pager_command(program_name(&pager), &pager_args);
                diff_viewer::wsl_pager_command_login(distro, workspace, &git, &pager, &git_args)
            },
        },
        Launch::Direct { program, args } => match wsl_program(ctx, distro, &program) {
            Some(path) => diff_viewer::wsl_direct_command(distro, workspace, &path, &args),
            None => diff_viewer::wsl_direct_command_login(
                distro,
                workspace,
                program_name(&program),
                &args,
            ),
        },
    }
}
```

In `git_panel.rs` tests, `git_row_diff_request` tests keep using `DiffSource` through the new import.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo nextest run -p alacritree --locked diff_viewer:: git_panel::`
Expected: PASS.

Run: `rg -n "build_(wsl_)?diff_command" alacritree/src`
Expected: no matches.

- [ ] **Step 6: Format, lint, commit**

```bash
devkit run task fmt && devkit run task clippy
git -C C:/Users/Lev/Git/github/alacritree-worktrees/feat-ux-configurable-diff-viewer add alacritree/src/diff_viewer.rs alacritree/src/main.rs alacritree/src/app/git_panel.rs
git -C C:/Users/Lev/Git/github/alacritree-worktrees/feat-ux-configurable-diff-viewer commit -m "refactor(diff): plan diff panes through a viewer" -m "The git panel built delta's argv inline, so another viewer meant a
second set of builders beside it. diff_viewer turns a viewer and a
target, one row or one section, into a launch and builds its native
or WSL argv. Delta becomes the first viewer, and the pane opens
exactly as before.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 6: `[integrations.diff_viewer]`, with doctor checking the viewer

**Files:**
- Modify: `alacritree/src/config.rs` to add `DiffViewerPreset`, `DiffViewerConfig`, `RawDiffViewer` and `RawCustomDiffViewer`, plus a field on `IntegrationsConfig` and `RawIntegrations`
- Modify: `alacritree/src/app/git_panel.rs` so `open_diff` uses the configured viewer
- Modify: `alacritree/src/cli/doctor.rs` to add `diff_viewer_check` and its test
- Regenerate: schema and stock config
- Modify: `docs/alacritree.md` `[integrations]` block

**Interfaces:**
- Consumes: `diff_viewer::{Viewer, Program, Templates}` (Task 5), `tools::program` (Task 2).
- Produces:
  - `pub enum config::DiffViewerPreset { Delta, Tuicr, Custom }`
  - `pub struct config::DiffViewerConfig { pub viewer: Viewer, pub section_buttons: bool, pub button_icon: String }`
  - `IntegrationsConfig::diff_viewer: DiffViewerConfig`

- [ ] **Step 1: Write the failing tests**

In `config.rs` tests:

```rust
    #[test]
    fn the_diff_viewer_defaults_to_delta_without_section_buttons() {
        let viewer = config_from("").integrations.diff_viewer;
        assert_eq!(viewer.viewer, Viewer::delta());
        assert!(!viewer.section_buttons);
        assert_eq!(viewer.button_icon, "review");
    }

    #[test]
    fn a_preset_names_its_viewer_and_an_unknown_one_is_delta() {
        let tuicr = config_from(
            "[integrations.diff_viewer]\npreset = \"tuicr\"\nsection_buttons = true\nbutton_icon = \"R\"\n",
        );
        assert_eq!(tuicr.integrations.diff_viewer.viewer, Viewer::tuicr());
        assert!(tuicr.integrations.diff_viewer.section_buttons);
        assert_eq!(tuicr.integrations.diff_viewer.button_icon, "R");

        let unknown = config_from("[integrations.diff_viewer]\npreset = \"meld\"\n");
        assert_eq!(unknown.integrations.diff_viewer.viewer, Viewer::delta());
    }

    #[test]
    fn a_custom_viewer_needs_exactly_one_mode() {
        let pager = config_from(
            "[integrations.diff_viewer]\npreset = \"custom\"\n\
             [integrations.diff_viewer.custom]\npager = \"delta --side-by-side\"\n\
             wsl_pager = \"/usr/bin/delta --side-by-side\"\n",
        );
        assert_eq!(pager.integrations.diff_viewer.viewer, Viewer::Pager {
            pager: Program::Custom {
                path: "delta --side-by-side".to_string(),
                wsl_path: Some("/usr/bin/delta --side-by-side".to_string()),
            },
            args: Vec::new(),
        });

        let direct = config_from(
            "[integrations.diff_viewer]\npreset = \"custom\"\n\
             [integrations.diff_viewer.custom]\npath = \"difft\"\nwsl_path = \"  \"\n\
             staged = [\"--staged\", \"{file}\"]\n",
        );
        assert_eq!(direct.integrations.diff_viewer.viewer, Viewer::Direct {
            program: Program::Custom { path: "difft".to_string(), wsl_path: None },
            templates: Templates {
                staged: vec!["--staged".to_string(), "{file}".to_string()],
                ..Templates::default()
            },
        });

        for both_or_neither in [
            "[integrations.diff_viewer.custom]\npager = \"delta\"\npath = \"difft\"\n",
            "[integrations.diff_viewer.custom]\n",
        ] {
            let toml = format!("[integrations.diff_viewer]\npreset = \"custom\"\n{both_or_neither}");
            assert_eq!(config_from(&toml).integrations.diff_viewer.viewer, Viewer::delta());
        }
    }
```

In `cli/doctor.rs` tests:

```rust
    /// A custom pager is a shell command line, not a program to look up.
    #[test]
    fn the_diff_viewer_check_looks_up_programs_only() {
        let pager = Viewer::Pager {
            pager: Program::Custom { path: "delta -s".to_string(), wsl_path: None },
            args: Vec::new(),
        };
        assert!(diff_viewer_check(&pager).is_none());

        let missing = Viewer::Direct {
            program: Program::Custom {
                path: "/definitely/not/here/tuicr".to_string(),
                wsl_path: None,
            },
            templates: Templates::default(),
        };
        let check = diff_viewer_check(&missing).expect("a direct viewer is checked");
        assert_eq!(check.name, "diff viewer");
        assert_eq!(check.status, Status::Warn);
    }
```

with `use crate::diff_viewer::{Program, Templates, Viewer};` in the doctor test module.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p alacritree --locked config::tests::the_diff_viewer config::tests::a_preset config::tests::a_custom_viewer doctor::tests::the_diff_viewer`
Expected: compile error, no field `diff_viewer` on `IntegrationsConfig` and no function `diff_viewer_check`.

- [ ] **Step 3: Implement the config**

In `config.rs`, add `use crate::diff_viewer::{Program, Templates, Viewer};`. Add `pub diff_viewer: DiffViewerConfig,` as the last field of `IntegrationsConfig`, and `diff_viewer: RawDiffViewer,` with doc `/// What the git panel's diff pane runs.` as the last field of `RawIntegrations`. In `RawIntegrations::resolve`, add `diff_viewer: self.diff_viewer.resolve(),`.

```rust
/// Which viewer `[integrations.diff_viewer]` runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, EnumIter, IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum DiffViewerPreset {
    #[default]
    Delta,
    Tuicr,
    Custom,
}

/// `[integrations.diff_viewer]`: what the git panel's diff pane runs, and
/// whether its section headers offer a whole-section review.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct DiffViewerConfig {
    pub viewer: Viewer,
    pub section_buttons: bool,
    pub button_icon: String,
}
```

```rust
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(default)]
struct RawDiffViewer {
    /// "delta" pipes git's diff through delta.  "tuicr" opens tuicr's review
    /// TUI, which saves each comment for agents to read.  "custom" runs
    /// `[integrations.diff_viewer.custom]`.
    #[schemars(schema_with = "closed_set_schema::<DiffViewerPreset>")]
    preset: String,
    /// Draw a button on each git panel section header that opens the whole
    /// section in the viewer.  The ReviewStaged, ReviewUnstaged and
    /// ReviewBranch actions work either way.
    section_buttons: bool,
    /// The glyph or word the section header button shows.
    button_icon: String,
    /// The viewer `preset = "custom"` runs.
    custom: RawCustomDiffViewer,
}

impl Default for RawDiffViewer {
    fn default() -> Self {
        Self {
            preset: "delta".to_string(),
            section_buttons: false,
            button_icon: "review".to_string(),
            custom: RawCustomDiffViewer::default(),
        }
    }
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(default)]
struct RawCustomDiffViewer {
    /// Pager mode: a command git runs as `core.pager` for the panel's own
    /// `git diff`.  Set this or `path`, never both.
    pager: String,
    /// Pager mode inside WSL: the command git runs as `core.pager` there.
    /// Empty runs `pager` through the distro's login shell.
    wsl_pager: String,
    /// Direct mode: a program that renders the diff itself, run with the
    /// argument list below that matches what was chosen.  An empty list
    /// makes that row kind or section open nothing.
    path: String,
    /// Direct mode inside WSL: the program to run there, as written.  Empty
    /// runs `path` through the distro's login shell, which finds a bare
    /// name.
    wsl_path: String,
    /// Arguments for a staged row.  `{file}` is the row's path.
    staged: Vec<String>,
    /// Arguments for an unstaged row.  `{file}` is the row's path.
    unstaged: Vec<String>,
    /// Arguments for an untracked row.  `{file}` is the row's path.
    untracked: Vec<String>,
    /// Arguments for a `Changes vs` row.  `{file}` is the row's path and
    /// `{base}` the branch it diffs against.
    branch: Vec<String>,
    /// Arguments for the Staged section header.
    staged_scope: Vec<String>,
    /// Arguments for the Unstaged section header.
    unstaged_scope: Vec<String>,
    /// Arguments for the `Changes vs` section header.  `{base}` is the
    /// branch it diffs against.
    branch_scope: Vec<String>,
}

impl RawDiffViewer {
    fn resolve(self) -> DiffViewerConfig {
        let preset = parse_closed_set("integrations.diff_viewer.preset", &self.preset);
        let viewer = match preset {
            DiffViewerPreset::Delta => Viewer::delta(),
            DiffViewerPreset::Tuicr => Viewer::tuicr(),
            DiffViewerPreset::Custom => self.custom.resolve().unwrap_or_else(|why| {
                log::warn!("[integrations.diff_viewer.custom] {why}; using the delta preset");
                Viewer::delta()
            }),
        };
        let button_icon = if self.button_icon.trim().is_empty() {
            RawDiffViewer::default().button_icon
        } else {
            self.button_icon
        };
        DiffViewerConfig { viewer, section_buttons: self.section_buttons, button_icon }
    }
}

impl RawCustomDiffViewer {
    fn resolve(self) -> Result<Viewer, &'static str> {
        let pager = self.pager.trim().to_string();
        let path = self.path.trim().to_string();
        let wsl = |value: String| Some(value.trim().to_string()).filter(|v| !v.is_empty());
        match (pager.is_empty(), path.is_empty()) {
            (false, true) => Ok(Viewer::Pager {
                pager: Program::Custom { path: pager, wsl_path: wsl(self.wsl_pager) },
                args: Vec::new(),
            }),
            (true, false) => Ok(Viewer::Direct {
                program: Program::Custom { path, wsl_path: wsl(self.wsl_path) },
                templates: Templates {
                    staged: self.staged,
                    unstaged: self.unstaged,
                    untracked: self.untracked,
                    branch: self.branch,
                    staged_scope: self.staged_scope,
                    unstaged_scope: self.unstaged_scope,
                    branch_scope: self.branch_scope,
                },
            }),
            (false, false) => Err("sets both pager and path"),
            (true, true) => Err("sets neither pager nor path"),
        }
    }
}
```

- [ ] **Step 4: The pane and doctor use it**

In `open_diff`, replace `diff_viewer::plan(&Viewer::delta(), &target)` with `diff_viewer::plan(&self.config.integrations.diff_viewer.viewer, &target)`, and drop `Viewer` from the `git_panel.rs` import if it is now unused.

In `cli/doctor.rs`, add `use crate::diff_viewer::{Program, Viewer};`, then add:

```rust
/// The program the configured diff viewer runs.  A custom pager is a shell
/// command line git hands to a shell, not a program to look up.
fn diff_viewer_check(viewer: &Viewer) -> Option<Check> {
    let program = match viewer {
        Viewer::Pager { pager: Program::Custom { .. }, .. } => return None,
        _ => match viewer.program() {
            Program::Tool(tool) => tools::program(*tool),
            Program::Custom { path, .. } => path.clone(),
        },
    };
    let tool = Tool {
        program: "diff viewer",
        consequence: "the git panel's diff pane opens an error instead of a diff",
        need: Need::Optional,
    };
    Some(tool_check(&tool, find(&program)))
}
```

In `report`, after `checks.extend(gh_auth_check());`:

```rust
    checks.extend(diff_viewer_check(&config.integrations.diff_viewer.viewer));
```

- [ ] **Step 5: Run the tests to verify they pass, then regenerate**

Run: `cargo nextest run -p alacritree --locked config:: doctor::`
Expected: PASS.

Run: `devkit run task test --env ALACRITREE_UPDATE_SCHEMA=1 --env ALACRITREE_UPDATE_STOCK=1`
Then: `devkit run task test`
Expected: PASS. The schema diff adds `RawDiffViewer` with defaults `"delta"`, `false` and `"review"`, and `RawCustomDiffViewer` with `""` and `[]` defaults. `schema-defaults-allowlist.txt` is unchanged.

- [ ] **Step 6: Document the keys**

In `docs/alacritree.md`, after the `[integrations.delta]` block from Task 3, add:

```toml
[integrations.diff_viewer]  # what the git panel's diff pane runs
preset          = "delta"   # "delta" pipes git's diff through delta; "tuicr"
                            # opens tuicr's review TUI on the clicked file,
                            # and agents read its comments with
                            # `tuicr review comments`; "custom" runs the table
                            # below
section_buttons = false     # draw a button on each git section header that
                            # opens the whole section in the viewer
button_icon     = "review"  # the glyph or word that button shows

[integrations.diff_viewer.custom]   # used when preset = "custom"
pager     = ""              # pager mode: a command git runs as core.pager.
                            # Set this or path, never both
wsl_pager = ""              # the same inside WSL; empty runs pager through
                            # the distro's login shell
path      = ""              # direct mode: a program that renders the diff
                            # itself, run with one argument list per target
wsl_path  = ""              # the same inside WSL, run as written; empty runs
                            # path through the distro's login shell
staged         = []         # rows: {file} is the row's path, and branch rows
unstaged       = []         # also get {base}, the branch the panel diffs
untracked      = []         # against
branch         = []
staged_scope   = []         # section headers: no {file}; branch_scope gets
unstaged_scope = []         # {base}. An empty list makes that row kind or
branch_scope   = []         # section open nothing
```

- [ ] **Step 7: Format, lint, commit**

```bash
devkit run task fmt && devkit run task clippy
git -C C:/Users/Lev/Git/github/alacritree-worktrees/feat-ux-configurable-diff-viewer add alacritree/src/config.rs alacritree/src/app/git_panel.rs alacritree/src/cli/doctor.rs schema/alacritree-config.json alacritree/tests/stock-config.json docs/alacritree.md
git -C C:/Users/Lev/Git/github/alacritree-worktrees/feat-ux-configurable-diff-viewer commit -m "feat(diff): choose the diff pane's viewer in config" -m "The diff pane only ever ran delta. [integrations.diff_viewer] picks
the delta or tuicr preset, or a custom viewer that is either a pager
for git or a program with one argument list per row kind and section.
A custom table that sets both modes or neither warns and falls back
to delta. doctor reports whether the viewer's program is installed.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 7: Section review buttons and Review actions

**Files:**
- Modify: `alacritree/src/bindings.rs`:
  - enum `:340-342`
  - `description` `:577`
  - `bindable_actions` `:1133, 1206`
  - `parse_action` `:1338`
  - tests `:2049-2058, 2118-2127`
- Modify: `alacritree/src/command_palette.rs:109`
- Modify: `alacritree/src/app/git_panel.rs`:
  - `GitSidebarView` `:58-75`
  - `GitSidebarRequests` `:77-81`
  - `git_sidebar_view` `:289-390`
  - `apply_git_sidebar_requests`
  - `paint_staged_section`, `paint_unstaged_section`, `paint_branch_section` and `section` `:650-810`
  - `dispatch_git_action` `:1010-1059`
  - tests
- Modify: `docs/alacritree.md` "Right sidebar" section and `docs/keyboard-shortcuts.md` after `SetBaseBranch` `:236-237`

**Interfaces:**
- Consumes: `Target`, `Section`, `diff_viewer::opens`, `DiffViewerConfig` (Tasks 5 and 6).
- Produces: `NamedAction::{ReviewStaged, ReviewUnstaged, ReviewBranch}`, `pub(super) fn git_panel::review_section(action: NamedAction, base: Option<&str>) -> Option<Section>`.

- [ ] **Step 1: Write the failing tests**

In `bindings.rs` tests, add `all.push(NamedAction::ReviewStaged); all.push(NamedAction::ReviewUnstaged); all.push(NamedAction::ReviewBranch);` to `every_new_action_round_trips_and_is_described`, and add the three to the `must ship without a default key` list in `default_bindings_cover_the_existing_filters_and_no_pr_filter`.

In `app/git_panel.rs` tests:

```rust
    #[test]
    fn a_review_action_names_its_section_and_the_branch_needs_a_base() {
        assert_eq!(review_section(NamedAction::ReviewStaged, None), Some(Section::Staged));
        assert_eq!(review_section(NamedAction::ReviewUnstaged, None), Some(Section::Unstaged));
        assert_eq!(review_section(NamedAction::ReviewBranch, None), None);
        assert_eq!(
            review_section(NamedAction::ReviewBranch, Some("main")),
            Some(Section::Branch { base: "main".to_string() })
        );
        assert_eq!(review_section(NamedAction::Paste, Some("main")), None);
    }

    /// An app whose current workspace shows a diff pane under `key`.  The
    /// session is never spawned, so no viewer runs.
    fn app_with_diff_pane(key: &str) -> AlacritreeApp {
        let workspace = PathBuf::from("C:/repo/wt");
        let (_, notify_rx) = std::sync::mpsc::channel();
        let mut app = AlacritreeApp::from_parts(
            Config::default(),
            Theme::from_config(&Config::default()),
            crate::state::PersistedState::default(),
            Vec::new(),
            (Vec::new(), crate::fonts::FaceMetrics::default()),
            notify_rx,
            (None, None),
        );
        let (session, _) = Session::pending_command(
            Context::default(),
            &app.config,
            Some(workspace.clone()),
            TermSize::new(80, 24),
            (8.0, 16.0),
            "git".to_string(),
            Vec::new(),
            "diff".to_string(),
            SessionKind::Diff { key: key.to_string() },
        );
        app.sessions.push(session);
        app.current_workspace = Some(workspace);
        app
    }

    fn has_diff_pane(app: &AlacritreeApp) -> bool {
        app.sessions.iter().any(|s| matches!(s.kind, SessionKind::Diff { .. }))
    }

    #[test]
    fn choosing_the_open_section_again_closes_its_pane() {
        let mut app = app_with_diff_pane("section:staged");
        app.open_diff(&Context::default(), Target::Section(Section::Staged));
        assert!(!has_diff_pane(&app));
    }

    #[test]
    fn a_section_the_viewer_cannot_open_leaves_the_open_pane_alone() {
        let mut app = app_with_diff_pane("staged:a.rs");
        app.config.integrations.diff_viewer.viewer = Viewer::Direct {
            program: Program::Custom { path: "difft".to_string(), wsl_path: None },
            templates: Templates::default(),
        };
        app.open_diff(&Context::default(), Target::Section(Section::Staged));
        assert!(has_diff_pane(&app));
    }
```

Import `Section`, `Templates` and `Viewer` from `crate::diff_viewer` in the test module if the file-level import no longer includes them.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p alacritree --locked bindings:: git_panel::`
Expected: compile error, no variant `ReviewStaged` and no function `review_section`.

- [ ] **Step 3: Add the actions**

`bindings.rs`, after `RefreshPrStatus` in the enum:

```rust
    /// Open the git panel's Staged section in the diff viewer, or close the
    /// pane when that section is already open.
    ReviewStaged,
    /// The same for the Unstaged section.
    ReviewUnstaged,
    /// The same for the `Changes vs` section.
    ReviewBranch,
```

In `description`:

```rust
            Self::ReviewStaged => "Review every staged change in the diff viewer".into(),
            Self::ReviewUnstaged => "Review every unstaged change in the diff viewer".into(),
            Self::ReviewBranch => "Review the branch against its base in the diff viewer".into(),
```

In `bindable_actions`, change the return type to `[NamedAction; 74]` and append `ReviewStaged, ReviewUnstaged, ReviewBranch,` after `RefreshPrStatus`. In `parse_action`, add:

```rust
        "ReviewStaged" => BindingAction::Named(ReviewStaged),
        "ReviewUnstaged" => BindingAction::Named(ReviewUnstaged),
        "ReviewBranch" => BindingAction::Named(ReviewBranch),
```

In `command_palette.rs` `section_of`, after the `SetBaseBranch` line:

```rust
        ReviewStaged | ReviewUnstaged | ReviewBranch => Workspaces,
```

In `git_panel.rs`, add to `dispatch_git_action`, before `_ => return false`:

```rust
            NamedAction::ReviewStaged | NamedAction::ReviewUnstaged | NamedAction::ReviewBranch => {
                if let Some(section) = review_section(action, self.cached_branch_base().as_deref()) {
                    self.open_diff(ctx, Target::Section(section));
                }
            },
```

In the same `impl AlacritreeApp` block:

```rust
    /// The base the `Changes vs` section diffs against, from the last status
    /// the workspace computed, so a palette action finds it while the git
    /// panel is hidden.
    fn cached_branch_base(&self) -> Option<String> {
        let path = self.active_session_path()?;
        let status = self.git_panel.status.get(&path)?.last();
        status.default_branch_resolved.clone().or_else(|| status.default_branch.clone())
    }
```

Next to `git_filter_identity`:

```rust
/// The section a Review action opens.  The branch section has nothing to
/// diff against until a base is known.
pub(super) fn review_section(action: NamedAction, base: Option<&str>) -> Option<Section> {
    match action {
        NamedAction::ReviewStaged => Some(Section::Staged),
        NamedAction::ReviewUnstaged => Some(Section::Unstaged),
        NamedAction::ReviewBranch => Some(Section::Branch { base: base?.to_string() }),
        _ => None,
    }
}
```

- [ ] **Step 4: Draw the buttons**

`GitSidebarRequests::diff` becomes `Option<Target>`. Rows set `requests.diff = Some(Target::Row(request));`, and `apply_git_sidebar_requests` calls `self.open_diff(ctx, target)`.

Add to `GitSidebarView`:

```rust
    /// The section header button's label.
    review_label: String,
    /// The whole-section review each header offers, `None` when section
    /// buttons are off or the viewer cannot open that section.
    staged_review: Option<Target>,
    unstaged_review: Option<Target>,
    branch_review: Option<Target>,
```

In `git_sidebar_view`, before building the view:

```rust
        let diff_viewer = &self.config.integrations.diff_viewer;
        let review = |section: Section| {
            let target = Target::Section(section);
            (diff_viewer.section_buttons && diff_viewer::opens(&diff_viewer.viewer, &target))
                .then_some(target)
        };
        let staged_review = review(Section::Staged);
        let unstaged_review = review(Section::Unstaged);
        let branch_review = git_branch_base.clone().and_then(|base| review(Section::Branch { base }));
        let review_label = diff_viewer.button_icon.clone();
```

and set the four fields in the `GitSidebarView` literal.

Add a header helper and a button type after `section`:

```rust
struct ReviewButton<'a> {
    label: &'a str,
    active: bool,
}

impl<'a> ReviewButton<'a> {
    fn for_target(view: &'a GitSidebarView, target: Option<&Target>) -> Option<Self> {
        let target = target?;
        let active = view.active_diff_key.as_deref() == Some(target.key().as_str());
        Some(Self { label: &view.review_label, active })
    }
}

/// A section header row.  Without a review button it is the plain
/// horizontal row the panel has always drawn; with one, the button pins to
/// the right edge.  Returns whether the button was clicked.
fn section_header(
    ui: &mut egui::Ui,
    theme: &Theme,
    review: Option<ReviewButton>,
    leading: impl FnOnce(&mut egui::Ui),
) -> bool {
    let Some(button) = review else {
        ui.horizontal(leading);
        return false;
    };
    let mut clicked = false;
    row_with_trailing(ui, leading, |ui| {
        let color = if button.active { theme.text } else { theme.text_muted };
        let resp = icon_tooltip(
            ui.add(
                egui::Label::new(RichText::new(button.label).color(color).small())
                    .selectable(false)
                    .sense(egui::Sense::click()),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand),
            "Review this section in the diff viewer",
            theme.icon_tooltips,
        );
        clicked = resp.clicked();
    });
    clicked
}
```

Change `section` to take the button and report its click:

```rust
fn section<R>(
    ui: &mut egui::Ui,
    theme: &Theme,
    title: &str,
    count: &SectionCount,
    filtering: bool,
    gap: &mut f32,
    review: Option<ReviewButton>,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> bool {
    if count.total == 0 {
        return false;
    }
    ui.add_space(std::mem::take(gap));
    let label = section_count_label(count, filtering);
    let clicked = section_header(ui, theme, review, |ui| {
        ui.label(RichText::new(title).color(theme.text).strong().small());
        ui.label(RichText::new(label).color(theme.text_muted).small());
    });
    ui.add_space(2.0);
    add_contents(ui);
    *gap = 10.0;
    clicked
}
```

If clippy flags `too_many_arguments` on `section`, pass `title`, `count` and `filtering` as one `SectionHeading<'_>` struct rather than adding an `allow`.

In `paint_staged_section`, the row loop is unchanged except that it sets `requests.diff = Some(Target::Row(request))`:

```rust
    let review = ReviewButton::for_target(view, view.staged_review.as_ref());
    let clicked = section(
        ui,
        &view.theme,
        "Staged",
        &view.staged_count,
        view.filtering,
        section_gap,
        review,
        |ui| {
            for file in &view.status.staged {
                if !view.staged_visible.contains(&file.path) {
                    continue;
                }
                let request = DiffRequest { file: file.path.clone(), source: DiffSource::Staged };
                let is_active = view.active_diff_key.as_deref() == Some(&diff_key(&request));
                let response = file_row(ui, file, &view.theme, is_active);
                if response.clicked() {
                    requests.diff = Some(Target::Row(request));
                }
                paint_git_row_cursor(
                    ui,
                    &response,
                    &view.cursor_row,
                    GitSection::Staged,
                    &file.path,
                    view.cursor_moved,
                    &view.theme,
                );
            }
        },
    );
    if clicked {
        requests.diff = view.staged_review.clone();
    }
```

`paint_unstaged_section` follows the same shape with `unstaged_review`. In `paint_branch_section`, replace the open-coded `ui.horizontal` header call with:

```rust
    let review = ReviewButton::for_target(view, view.branch_review.as_ref());
    let clicked = section_header(ui, &view.theme, review, |ui| {
        ui.label(RichText::new(&base_label).color(view.theme.text).strong().small());
        if let Some(pr) = &view.pr_info {
            ui.label(RichText::new("·").color(view.theme.text_muted).small());
            ui.hyperlink_to(
                RichText::new(format!("PR #{}", pr.number))
                    .color(view.theme.accent)
                    .small()
                    .strong(),
                &pr.url,
            );
        }
        ui.label(RichText::new(count_label).color(view.theme.text_muted).small());
    });
    if clicked {
        requests.diff = view.branch_review.clone();
    }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `devkit run task test`
Expected: PASS, including `choosing_the_open_section_again_closes_its_pane` and `a_section_the_viewer_cannot_open_leaves_the_open_pane_alone`.

If the test module cannot reach `open_diff`, change it to `pub(super)`.

- [ ] **Step 6: Try it by hand**

Run: `devkit run task build`, then launch the debug binary with an `alacritree.toml` holding `[integrations.diff_viewer]` `section_buttons = true`. Check each of these:

1. A worktree with staged, unstaged and branch changes shows `review` right-aligned on all three section headers.
2. Clicking the Staged `review` opens a pane titled `diff: staged changes` running `git diff --cached` through delta, and the label turns bright. Clicking it again closes the pane.
3. Clicking a file row while a section pane is open replaces the pane with that file's diff.
4. Ctrl+K `ReviewUnstaged` opens the unstaged pane with the git sidebar hidden.
5. With `section_buttons = false`, the headers look exactly as they did before, with no taller rows.
6. With `preset = "tuicr"` and tuicr installed, a staged row opens `tuicr -w -p <file>`, and the Changes vs header opens `tuicr -r <base>...HEAD`.

- [ ] **Step 7: Document the actions**

`docs/keyboard-shortcuts.md`, after the `SetBaseBranch` entry:

```markdown
- `ReviewStaged` / `ReviewUnstaged` / `ReviewBranch`: open the git panel's
  Staged, Unstaged, or `Changes vs` section as a whole in the diff viewer, or
  close that pane when it is already open. No default keys.
```

`docs/alacritree.md`, at the end of the "Right sidebar" git status section before `### Per-worktree base branch`:

```markdown
Clicking a file opens its diff in a pane, and clicking it again closes the
pane. `[integrations.diff_viewer]` picks what that pane runs: delta by default,
tuicr for a review whose comments agents can read, or a custom command. With
`section_buttons = true` each section header also gets a button that opens the
whole section at once; the `ReviewStaged`, `ReviewUnstaged` and `ReviewBranch`
actions do the same from the palette or a key binding.
```

- [ ] **Step 8: Format, lint, commit**

```bash
devkit run task fmt && devkit run task clippy
git -C C:/Users/Lev/Git/github/alacritree-worktrees/feat-ux-configurable-diff-viewer add alacritree/src/bindings.rs alacritree/src/command_palette.rs alacritree/src/app/git_panel.rs docs/alacritree.md docs/keyboard-shortcuts.md
git -C C:/Users/Lev/Git/github/alacritree-worktrees/feat-ux-configurable-diff-viewer commit -m "feat(git-panel): review a whole section at once" -m "The panel could only open one file at a time, which suits delta but
not a review tool like tuicr. Each section header can carry a review
button, opt-in through section_buttons, that opens the whole section
in the configured viewer. The ReviewStaged, ReviewUnstaged and
ReviewBranch actions do the same without the buttons.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Settled decisions

1. **Each side has its own path.** `path` is the native program and `wsl_path` the program inside WSL, empty meaning discovery by name. A Windows path never reaches a distro. The custom viewer has the same split through `wsl_pager` and `wsl_path`.
2. **git inside WSL scripts is out of scope.** Batch scripts run by the resident helper's plain `sh` find `git` on the distro's PATH on every run; the hello resolves git too, but nothing reads that path. `[integrations.git]` governs only spawns that name the program in argv, and the `RawGit` doc says so.
3. **Unavailable rows do nothing.** A row whose custom template is empty stays clickable and a click opens nothing.

## Unresolved questions

None.
