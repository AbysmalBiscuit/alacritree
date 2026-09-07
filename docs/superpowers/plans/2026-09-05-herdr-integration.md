# herdr Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** agents running under a herdr server appear in alacritree's left sidebar under the worktree they are working in, with live status, and Enter opens one in an ordinary alacritree session.

**Architecture:** A new `herdr.rs` owns discovery, JSON parsing, path translation and command construction, and holds no egui types so every decision in it is unit-testable. A per-endpoint cache polls `herdr agent list` on the `jobs.rs` pool and exposes results to the sidebar through a generation counter, the same way PR lookups already reach the reconcile. Attaching spawns an ordinary session whose program is a herdr attach command; alacritree never owns a herdr PTY and herdr never owns alacritree's.

**Tech Stack:** Rust 2024 (MSRV 1.85), egui/eframe, `serde`/`serde_json`, `toml`, `schemars`. herdr 0.8.2 is an external CLI, not a dependency. No new crates.

**Spec:** `docs/superpowers/specs/2026-09-05-herdr-integration-design.md`. Read it before Task 1; this plan argues from it and does not restate its reasoning.

## Global Constraints

- **Base branch.** Cut `feat/herdr-integration` from PR 210's tip (`fix/wsl-helper-liveness`, marker `[8]`), not from `master`. `alacritree/src/jobs.rs` exists only there and Tasks 5 and 9 depend on it. Read the open-PR list fresh at setup; the tip moves.
- **Line numbers drift; symbol names do not.** Citations below give the base branch's line where it is known and `master`'s otherwise, and the stack moves under both. Locate every cited item by its name — `spawn_session_with_shell`, `reap_exited_sessions` — and treat a mismatched line as stale, not as the wrong function.
- **Test command.** `cargo nextest run -p alacritree` in this checkout. Not `cargo test`.
- **No test invokes a real herdr binary.** CI has none. Every test runs on captured fixtures.
- **All work lives in `alacritree/`.** The vendored alacritty crates are read-only.
- **Comments explain why, never what.** No PR references, no task numbers, no `this now does X` phrasing. A comment that restates the line next to it gets deleted.
- **Config keys carry doc comments** on their `Raw*` struct in `config.rs`; those comments are the published JSON Schema's hover text. Regenerate with `ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema` or the build fails on a stale schema.
- **Conventional Commits**, imperative subject under ~72 chars, with the trailer `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.
- **herdr writes success to stdout and errors to stderr.** Any code reading only stdout is wrong.

---

### Task 1: herdr wire types and parsing

**Files:**
- Create: `alacritree/src/herdr.rs`
- Modify: `alacritree/src/main.rs` (add `mod herdr;`)

**Interfaces:**
- Consumes: nothing.
- Produces: `herdr::Side`, `herdr::Status`, `herdr::Agent`, `herdr::parse_agent_list(stdout: &str) -> Vec<Agent>`, `herdr::error_code(stderr: &str) -> Option<String>`.

- [ ] **Step 1: Write the failing test**

Create `alacritree/src/herdr.rs` with only this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from a native Windows server.  `skip_serializing_if` drops
    /// `foreground_cwd`, `name`, `display_agent` and `agent_session` rather
    /// than emitting them as null.
    const WINDOWS: &str = r#"{"id":"cli:agent:list","result":{"agents":[
        {"agent":"claude","agent_status":"idle","pane_id":"w5:p1",
         "terminal_id":"term_65abfc8e300361","revision":7,"state_change_seq":3,
         "cwd":"C:\\Users\\Lev\\Git\\github\\alacritree","focused":true,
         "tab_id":"w5:t1","workspace_id":"w5"}],"type":"agent_list"}}"#;

    #[test]
    fn parses_a_windows_agent_with_absent_optional_fields() {
        let agents = parse_agent_list(WINDOWS);
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].terminal_id, "term_65abfc8e300361");
        assert_eq!(agents[0].pane_id, "w5:p1");
        assert_eq!(agents[0].kind.as_deref(), Some("claude"));
        assert_eq!(agents[0].status, Status::Idle);
        assert_eq!(agents[0].foreground_cwd, None);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p alacritree herdr::`
Expected: compile error, `cannot find function parse_agent_list`.

- [ ] **Step 3: Write minimal implementation**

Above the test module in `alacritree/src/herdr.rs`:

```rust
//! Surface agents running under a herdr server in the sidebar.
//!
//! herdr owns its own PTYs and detects the agent in each pane; alacritree
//! only asks what it has and can hand one to a shell.  Everything here goes
//! through the `herdr` CLI rather than its socket, so a missing binary or an
//! absent server is a silent no-op and no wire protocol is pinned.  herdr
//! prints success on stdout and errors on stderr, which is why callers
//! capture both.

use serde::Deserialize;

/// Which herdr server an agent belongs to.  Two servers on one machine
/// cannot see each other, so this is part of an agent's identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Side {
    Native,
    /// Named distro, as `wsl.exe -d` spells it.
    Wsl(String),
}

/// herdr's agent state.  An unrecognised string maps to `Unknown` so a value
/// herdr adds later renders as a plain row instead of dropping the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    Idle,
    Working,
    Blocked,
    Done,
    #[default]
    Unknown,
}

impl Status {
    fn parse(raw: &str) -> Self {
        match raw {
            "idle" => Self::Idle,
            "working" => Self::Working,
            "blocked" => Self::Blocked,
            "done" => Self::Done,
            _ => Self::Unknown,
        }
    }
}

/// One agent as herdr reports it.  `terminal_id` is the identity because
/// `pane_id` is positional: a pane moved between workspaces gets a new one,
/// and ids restart at `w1` after `session delete`.
#[derive(Debug, Clone)]
pub struct Agent {
    pub terminal_id: String,
    pub pane_id: String,
    pub kind: Option<String>,
    pub status: Status,
    pub cwd: Option<String>,
    pub foreground_cwd: Option<String>,
    pub state_change_seq: u64,
}

#[derive(Deserialize)]
struct Envelope {
    result: Option<AgentList>,
}

#[derive(Deserialize)]
struct AgentList {
    #[serde(default)]
    agents: Vec<RawAgent>,
}

/// Only the fields the sidebar renders.  Everything else herdr sends is
/// ignored, so an additive protocol change costs nothing.
#[derive(Deserialize)]
struct RawAgent {
    terminal_id: Option<String>,
    pane_id: Option<String>,
    agent_status: Option<String>,
    agent: Option<String>,
    display_agent: Option<String>,
    cwd: Option<String>,
    foreground_cwd: Option<String>,
    #[serde(default)]
    state_change_seq: u64,
}

/// Agents from one `herdr agent list` reply.  An agent missing an identity
/// or a status is dropped on its own; its siblings still parse.
pub fn parse_agent_list(stdout: &str) -> Vec<Agent> {
    let Ok(envelope) = serde_json::from_str::<Envelope>(stdout) else {
        return Vec::new();
    };
    let Some(list) = envelope.result else {
        return Vec::new();
    };
    list.agents
        .into_iter()
        .filter_map(|raw| {
            Some(Agent {
                terminal_id: raw.terminal_id?,
                pane_id: raw.pane_id?,
                status: Status::parse(&raw.agent_status?),
                kind: raw.display_agent.or(raw.agent),
                cwd: raw.cwd,
                foreground_cwd: raw.foreground_cwd,
                state_change_seq: raw.state_change_seq,
            })
        })
        .collect()
}

/// The `code` from an error envelope on stderr, for deciding whether a
/// failure is the ordinary "no server" case or worth a log line.
pub fn error_code(stderr: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct ErrEnvelope {
        error: ErrBody,
    }
    #[derive(Deserialize)]
    struct ErrBody {
        code: String,
    }
    serde_json::from_str::<ErrEnvelope>(stderr).ok().map(|e| e.error.code)
}
```

Add `mod herdr;` to `alacritree/src/main.rs` beside the other module declarations.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p alacritree herdr::`
Expected: PASS.

- [ ] **Step 5: Add the remaining parsing tests**

Append to the test module:

```rust
    /// Captured from a WSL server, which does populate `foreground_cwd`.
    const WSL: &str = r#"{"id":"cli:agent:list","result":{"agents":[
        {"agent":"codex","agent_status":"idle","pane_id":"w4:p1",
         "terminal_id":"term_65ab9ae95a74d2","revision":9,"state_change_seq":9,
         "cwd":"/home/lev/Git/lev/devkit","foreground_cwd":"/home/lev/Git/lev/devkit",
         "focused":true,"tab_id":"w4:t1","workspace_id":"w4"}],"type":"agent_list"}}"#;

    #[test]
    fn parses_a_wsl_agent_with_foreground_cwd() {
        let agents = parse_agent_list(WSL);
        assert_eq!(agents[0].foreground_cwd.as_deref(), Some("/home/lev/Git/lev/devkit"));
        assert_eq!(agents[0].kind.as_deref(), Some("codex"));
    }

    #[test]
    fn empty_agent_list_is_not_an_error() {
        let reply = r#"{"id":"cli:agent:list","result":{"agents":[],"type":"agent_list"}}"#;
        assert!(parse_agent_list(reply).is_empty());
    }

    #[test]
    fn unknown_fields_and_unknown_status_survive() {
        let reply = r#"{"id":"x","surprise":1,"result":{"agents":[
            {"terminal_id":"t1","pane_id":"w1:p1","agent_status":"meditating",
             "future_field":true}],"type":"agent_list"}}"#;
        let agents = parse_agent_list(reply);
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].status, Status::Unknown);
    }

    #[test]
    fn an_agent_without_an_identity_is_dropped_alone() {
        let reply = r#"{"id":"x","result":{"agents":[
            {"pane_id":"w1:p1","agent_status":"idle"},
            {"terminal_id":"t2","pane_id":"w1:p2","agent_status":"idle"}],"type":"agent_list"}}"#;
        let agents = parse_agent_list(reply);
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].terminal_id, "t2");
    }

    #[test]
    fn display_agent_wins_over_agent() {
        let reply = r#"{"id":"x","result":{"agents":[
            {"terminal_id":"t1","pane_id":"w1:p1","agent_status":"idle",
             "agent":"claude","display_agent":"Claude Code"}],"type":"agent_list"}}"#;
        assert_eq!(parse_agent_list(reply)[0].kind.as_deref(), Some("Claude Code"));
    }

    /// The reply that arrives on stderr with stdout empty when no server is
    /// listening.  A parser reading only stdout never sees this.
    #[test]
    fn reads_the_error_code_off_stderr() {
        let stderr = r#"{"error":{"code":"server_not_running","message":"no herdr server"},"id":"cli:agent:list"}"#;
        assert_eq!(error_code(stderr).as_deref(), Some("server_not_running"));
        assert!(parse_agent_list("").is_empty());
    }
```

- [ ] **Step 6: Run the tests**

Run: `cargo nextest run -p alacritree herdr::`
Expected: PASS, 7 tests.

- [ ] **Step 7: Commit**

```bash
git add alacritree/src/herdr.rs alacritree/src/main.rs
git commit -m "feat(herdr): parse herdr agent listings"
```

---

### Task 2: Command construction per side

**Files:**
- Modify: `alacritree/src/herdr.rs`

**Interfaces:**
- Consumes: `Side` from Task 1.
- Produces: `Side::command(&self, args: &[&str]) -> (String, Vec<String>)`, `herdr::attach_args(pane_id: &str) -> Vec<String>`, `herdr::session_attach_script(pane_id: &str, session: &str) -> String`.

- [ ] **Step 1: Write the failing test**

Append to the test module in `alacritree/src/herdr.rs`:

```rust
    #[test]
    fn native_runs_herdr_directly() {
        let (program, args) = Side::Native.command(&["agent", "list"]);
        assert_eq!(program, "herdr");
        assert_eq!(args, vec!["agent", "list"]);
    }

    /// herdr installs to ~/.local/bin, which reaches PATH only under a login
    /// shell.  `wsl.exe -e herdr` fails with execvpe ENOENT.
    #[test]
    fn wsl_wraps_in_a_login_shell() {
        let (program, args) = Side::Wsl("kali-linux".into()).command(&["agent", "list"]);
        assert_eq!(program, "wsl.exe");
        assert_eq!(args, vec!["-d", "kali-linux", "--exec", "sh", "-lc", "herdr agent list"]);
    }

    #[test]
    fn wsl_quotes_arguments_that_need_it() {
        let (_, args) = Side::Wsl("d".into()).command(&["agent", "attach", "w1:p1"]);
        assert_eq!(args.last().unwrap(), "herdr agent attach 'w1:p1'");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p alacritree herdr::`
Expected: compile error, `no method named command`.

- [ ] **Step 3: Write minimal implementation**

Add to `alacritree/src/herdr.rs`:

```rust
/// Single-quote a POSIX argument, since WSL invocations are one `sh -lc`
/// string rather than an argv.
fn sh_quote(arg: &str) -> String {
    if !arg.is_empty() && arg.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:=".contains(c)) {
        return arg.to_string();
    }
    format!("'{}'", arg.replace('\'', r"'\''"))
}

impl Side {
    /// Program and argv that run `herdr <args>` on this side.  WSL goes
    /// through a login shell because herdr lives in `~/.local/bin`, which is
    /// not on the PATH `wsl.exe -e` inherits.
    pub fn command(&self, args: &[&str]) -> (String, Vec<String>) {
        match self {
            Self::Native => {
                ("herdr".to_string(), args.iter().map(|a| (*a).to_string()).collect())
            },
            Self::Wsl(distro) => {
                let script = std::iter::once("herdr".to_string())
                    .chain(args.iter().map(|a| sh_quote(a)))
                    .collect::<Vec<_>>()
                    .join(" ");
                (
                    "wsl.exe".to_string(),
                    vec![
                        "-d".to_string(),
                        distro.clone(),
                        "--exec".to_string(),
                        "sh".to_string(),
                        "-lc".to_string(),
                        script,
                    ],
                )
            },
        }
    }
}

/// Direct attach to one agent.  Unsupported on native Windows, where
/// `run_terminal_attach` is a `#[cfg(windows)]` refusal.
pub fn attach_args(pane_id: &str) -> Vec<String> {
    vec!["agent".into(), "attach".into(), pane_id.into()]
}

/// The native-Windows fallback: focus the pane, then attach to the whole
/// herdr session.  Two commands, so this returns a shell line rather than an
/// argv.  herdr's focus is server-global, so this moves the pane the user's
/// own herdr window is showing.
pub fn session_attach_script(pane_id: &str, session: &str) -> String {
    format!(
        "herdr agent focus {} && herdr session attach {}",
        sh_quote(pane_id),
        sh_quote(session)
    )
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p alacritree herdr::`
Expected: PASS.

- [ ] **Step 5: Test the attach helpers**

```rust
    #[test]
    fn direct_attach_targets_the_pane_id() {
        assert_eq!(attach_args("w5:p1"), vec!["agent", "attach", "w5:p1"]);
    }

    #[test]
    fn the_windows_fallback_focuses_then_attaches_the_session() {
        assert_eq!(
            session_attach_script("w5:p1", "default"),
            "herdr agent focus 'w5:p1' && herdr session attach default"
        );
    }
```

- [ ] **Step 6: Run the tests**

Run: `cargo nextest run -p alacritree herdr::`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add alacritree/src/herdr.rs
git commit -m "feat(herdr): build per-side herdr commands"
```

---

### Task 3: `[ui.herdr]` config

**Files:**
- Modify: `alacritree/src/config.rs`
- Modify: `schema/alacritree-config.json` (regenerated, not hand-edited)

**Interfaces:**
- Consumes: nothing.
- Produces: `config.ui.herdr: HerdrConfig { enabled: bool, poll_interval: Duration, show_unmatched: bool }`.

- [ ] **Step 1: Write the failing test**

In `alacritree/src/config.rs`'s test module:

```rust
#[test]
fn herdr_defaults_to_enabled_with_a_two_second_poll() {
    let config = Config::default();
    assert!(config.ui.herdr.enabled);
    assert_eq!(config.ui.herdr.poll_interval, Duration::from_millis(2000));
    assert!(config.ui.herdr.show_unmatched);
}

#[test]
fn herdr_can_be_turned_off() {
    let toml = "[ui.herdr]\nenabled = false\npoll_interval_ms = 5000\n";
    let config = config_from_str(toml);
    assert!(!config.ui.herdr.enabled);
    assert_eq!(config.ui.herdr.poll_interval, Duration::from_millis(5000));
}
```

If `config_from_str` does not exist under that name, use whichever helper the neighbouring config tests already use to build a `Config` from a TOML string.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p alacritree config::`
Expected: compile error, no field `herdr`.

- [ ] **Step 3: Write minimal implementation**

Add the resolved type near the other `ui` types:

```rust
#[derive(Debug, Clone)]
pub struct HerdrConfig {
    pub enabled: bool,
    pub poll_interval: Duration,
    pub show_unmatched: bool,
}

impl Default for HerdrConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval: Duration::from_millis(2000),
            show_unmatched: true,
        }
    }
}
```

Add `pub herdr: HerdrConfig` to the `UiTheme` struct (`config.rs:32` is where `ui` is held; the field goes on the struct `ui` resolves to) and `herdr: HerdrConfig::default()` to its `Default`.

Add the raw table beside `RawUiWsl` and friends:

```rust
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(default)]
struct RawHerdr {
    /// Discover herdr servers and list their agents in the sidebar.  Inert
    /// when no herdr binary or server is present.
    enabled: Option<bool>,
    /// How often a reachable herdr server is re-polled for agent state.
    poll_interval_ms: Option<u64>,
    /// List agents whose working directory matches no worktree, under Home.
    show_unmatched: Option<bool>,
}
```

Add `herdr: RawHerdr` to `RawUi`, and in the place `RawUi` is resolved into `UiTheme`:

```rust
herdr: HerdrConfig {
    enabled: self.ui.herdr.enabled.unwrap_or(true),
    poll_interval: Duration::from_millis(self.ui.herdr.poll_interval_ms.unwrap_or(2000)),
    show_unmatched: self.ui.herdr.show_unmatched.unwrap_or(true),
},
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p alacritree config::`
Expected: PASS.

- [ ] **Step 5: Regenerate the schema**

Run: `ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema`
Then run it again without the variable to confirm it is clean:
Run: `cargo test -p alacritree --test config_schema`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add alacritree/src/config.rs schema/alacritree-config.json
git commit -m "feat(config): add the ui.herdr table"
```

---

### Task 4: Resolve herdr inside WSL through the helper hello

**Files:**
- Modify: `alacritree/src/wsl_helper.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `wsl_helper::Capabilities.herdr: Option<String>`, `wsl_helper::capability_herdr(distro: &str) -> Option<String>`.

The helper's `RUN` branch runs `sh -c`, not a login shell, which is why git, delta and `gh` are resolved once at hello time. herdr joins them rather than paying a login shell per poll.

- [ ] **Step 1: Write the failing test**

In `wsl_helper.rs`'s test module, beside the existing `HELLO_LINE` tests:

```rust
#[test]
fn parses_a_herdr_path_from_the_hello() {
    let git = B64.encode("/usr/bin/git");
    let herdr = B64.encode("/home/lev/.local/bin/herdr");
    let rt = B64.encode("/run/user/1000/alacritree");
    let line = format!("hello\t2\t{git}\t\t\t{herdr}\t{rt}\n");
    let caps = parse_hello(&line).expect("hello should parse");
    assert_eq!(caps.herdr.as_deref(), Some("/home/lev/.local/bin/herdr"));
    assert_eq!(caps.git.as_deref(), Some("/usr/bin/git"));
    assert_eq!(caps.delta, None);
}

#[test]
fn a_distro_without_herdr_reports_none() {
    let rt = B64.encode("/tmp/alacritree");
    let line = format!("hello\t2\t\t\t\t\t{rt}\n");
    assert_eq!(parse_hello(&line).expect("hello should parse").herdr, None);
}

/// The old five-field hello is not silently accepted with a shifted
/// runtime_dir; a stale helper is torn down and respawned instead.
#[test]
fn the_previous_protocol_version_is_rejected() {
    assert!(parse_hello("hello\t1\t\t\t\t\n").is_none());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p alacritree wsl_helper::`
Expected: compile error, no field `herdr` on `Capabilities`.

- [ ] **Step 3: Write minimal implementation**

Add the field to `Capabilities`:

```rust
pub struct Capabilities {
    pub git: Option<String>,
    pub delta: Option<String>,
    pub gh: Option<String>,
    pub herdr: Option<String>,
    pub runtime_dir: String,
}
```

Bump `PROTOCOL_VERSION` from `"1"` to `"2"`, and decode the new field in `parse_hello` between `gh` and `runtime_dir`:

```rust
    let git = decode()?;
    let delta = decode()?;
    let gh = decode()?;
    let herdr = decode()?;
    let runtime_dir = decode()?;
    let some = |s: String| (!s.is_empty()).then_some(s);
    Some(Capabilities {
        git: some(git),
        delta: some(delta),
        gh: some(gh),
        herdr: some(herdr),
        runtime_dir,
    })
```

In `HELPER_SCRIPT`, add herdr to the capability probe and to the hello line:

```sh
caps=$("$s" -lc 'command -v git || echo; command -v delta || echo; command -v gh || echo; command -v herdr || echo' 2>/dev/null)
rt=${XDG_RUNTIME_DIR:-/tmp}/alacritree
printf 'hello\t2\t%s\t%s\t%s\t%s\t%s\n' \
  "$(b64 "$(printf %s "$caps" | sed -n 1p)")" \
  "$(b64 "$(printf %s "$caps" | sed -n 2p)")" \
  "$(b64 "$(printf %s "$caps" | sed -n 3p)")" \
  "$(b64 "$(printf %s "$caps" | sed -n 4p)")" \
  "$(b64 "$rt")"
```

Add the accessor beside `capability_gh`:

```rust
pub fn capability_herdr(distro: &str) -> Option<String> {
    client(distro)?.capabilities()?.herdr.clone()
}
```

Update the existing `HELLO_LINE` test constant to `"hello\t2\t\t\t\t\t\n"` and any other test that spells a hello literally.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p alacritree wsl_helper::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add alacritree/src/wsl_helper.rs
git commit -m "feat(wsl): resolve herdr in the helper hello"
```

---

### Task 5: Endpoint polling with two-tier backoff

**Files:**
- Modify: `alacritree/src/herdr.rs`

**Interfaces:**
- Consumes: `Side`, `Agent`, `parse_agent_list`, `error_code` (Task 1), `Side::command` (Task 2), `jobs::pool`, `jobs::Priority`, `jobs::Job`.
- Produces: `herdr::EndpointCache::new(Side) -> EndpointCache`, `EndpointCache::poll(&mut self, interval: Duration)`, `EndpointCache::agents(&self) -> &[Agent]`, `EndpointCache::generation(&self) -> u64`, `herdr::Reach` (the backoff state machine).

Backoff is two-tier because a permanent disable strands every user who runs `herdr update`, which restarts the server.

- [ ] **Step 1: Write the failing test**

Append to `herdr.rs`'s test module:

```rust
    use std::time::Duration;

    #[test]
    fn an_endpoint_that_never_answered_is_given_up_on() {
        let mut reach = Reach::default();
        reach.record_failure("server_not_running");
        assert!(!reach.should_retry(Duration::from_secs(3600)));
    }

    #[test]
    fn an_endpoint_that_answered_once_keeps_retrying() {
        let mut reach = Reach::default();
        reach.record_success();
        reach.record_failure("server_not_running");
        assert!(!reach.should_retry(Duration::from_secs(5)));
        assert!(reach.should_retry(Duration::from_secs(31)));
    }

    #[test]
    fn a_recovered_endpoint_polls_at_the_normal_interval_again() {
        let mut reach = Reach::default();
        reach.record_success();
        reach.record_failure("server_not_running");
        reach.record_success();
        assert!(reach.should_retry(Duration::from_secs(0)));
    }

    #[test]
    fn a_repeated_error_is_logged_once() {
        let mut reach = Reach::default();
        assert!(reach.record_failure("protocol_mismatch"));
        assert!(!reach.record_failure("protocol_mismatch"));
        assert!(reach.record_failure("server_not_running"));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p alacritree herdr::`
Expected: compile error, `cannot find type Reach`.

- [ ] **Step 3: Write minimal implementation**

```rust
use std::time::{Duration, Instant};

/// How long a server that has answered before waits before being retried.
const RECOVERY_RETRY: Duration = Duration::from_secs(30);

/// Whether an endpoint is worth talking to.  An endpoint that has never
/// answered is abandoned, so a machine with no herdr pays one failed spawn
/// rather than one per tick; an endpoint that answered and then stopped is
/// retried forever, because `herdr update` restarts the server.
#[derive(Debug, Default)]
pub struct Reach {
    ever_answered: bool,
    failing: bool,
    last_error: Option<String>,
}

impl Reach {
    /// Whether to poll again, given how long it has been since the last try.
    pub fn should_retry(&self, since_last: Duration) -> bool {
        match (self.failing, self.ever_answered) {
            (false, _) => true,
            (true, true) => since_last >= RECOVERY_RETRY,
            (true, false) => false,
        }
    }

    pub fn record_success(&mut self) {
        self.ever_answered = true;
        self.failing = false;
        self.last_error = None;
    }

    /// Records a failure, returning whether it is worth logging — a code
    /// repeating every tick is logged once, not once per poll.
    pub fn record_failure(&mut self, code: &str) -> bool {
        self.failing = true;
        let novel = self.last_error.as_deref() != Some(code);
        self.last_error = Some(code.to_string());
        novel
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p alacritree herdr::`
Expected: PASS.

- [ ] **Step 5: Add the cache around it**

```rust
/// One herdr server's agents, refreshed off the UI thread.
pub struct EndpointCache {
    side: Side,
    agents: Vec<Agent>,
    generation: u64,
    reach: Reach,
    last_attempt: Option<Instant>,
    pending: Option<jobs::Job<Result<Vec<Agent>, String>>>,
}

impl EndpointCache {
    pub fn new(side: Side) -> Self {
        Self {
            side,
            agents: Vec::new(),
            generation: 0,
            reach: Reach::default(),
            last_attempt: None,
            pending: None,
        }
    }

    /// Bumped only when a rendered field changes, so the sidebar's per-frame
    /// comparison does not rebuild for `revision` churn nobody can see.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn agents(&self) -> &[Agent] {
        &self.agents
    }

    /// Adopts a landed result and starts a new poll when due.  Never blocks.
    pub fn poll(&mut self, interval: Duration) {
        if let Some(job) = &self.pending {
            match job.poll() {
                Some(Ok(agents)) => {
                    self.reach.record_success();
                    if rendered_differs(&self.agents, &agents) {
                        self.generation = self.generation.wrapping_add(1);
                    }
                    self.agents = agents;
                    self.pending = None;
                },
                Some(Err(code)) => {
                    if self.reach.record_failure(&code) && code != "server_not_running" {
                        log::warn!("herdr ({:?}): {code}", self.side);
                    }
                    if !self.agents.is_empty() {
                        self.agents.clear();
                        self.generation = self.generation.wrapping_add(1);
                    }
                    self.pending = None;
                },
                None if job.failed() => self.pending = None,
                None => return,
            }
        }

        let since = self.last_attempt.map_or(interval, |t| t.elapsed());
        if since < interval || !self.reach.should_retry(since) {
            return;
        }
        self.last_attempt = Some(Instant::now());
        let side = self.side.clone();
        self.pending =
            Some(jobs::pool().spawn(jobs::Priority::Background, move |_| list_agents(&side)));
    }
}

/// Whether anything the sidebar draws changed.  `revision` and
/// `state_change_seq` deliberately do not count: they move on output the row
/// does not show.
fn rendered_differs(was: &[Agent], now: &[Agent]) -> bool {
    was.len() != now.len()
        || was.iter().zip(now).any(|(a, b)| {
            a.terminal_id != b.terminal_id
                || a.status != b.status
                || a.kind != b.kind
                || a.cwd != b.cwd
                || a.foreground_cwd != b.foreground_cwd
                || a.pane_id != b.pane_id
        })
}
```

Add the subprocess call, capturing both streams:

```rust
use std::process::{Command, Stdio};

use crate::command_ext::CommandExt;

/// Runs `herdr agent list` on one side.  Success is on stdout, errors are on
/// stderr, so both are captured; the exit status decides which to read.
fn list_agents(side: &Side) -> Result<Vec<Agent>, String> {
    let (program, args) = side.command(&["agent", "list"]);
    let output = Command::new(program)
        .hide_console()
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|_| "spawn_failed".to_string())?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(error_code(&stderr).unwrap_or_else(|| "server_not_running".to_string()));
    }
    Ok(parse_agent_list(&String::from_utf8_lossy(&output.stdout)))
}
```

- [ ] **Step 6: Test the generation counter**

```rust
    fn agent(id: &str, status: Status) -> Agent {
        Agent {
            terminal_id: id.into(),
            pane_id: "w1:p1".into(),
            kind: Some("claude".into()),
            status,
            cwd: Some("/repo".into()),
            foreground_cwd: None,
            state_change_seq: 0,
        }
    }

    #[test]
    fn churn_the_sidebar_cannot_see_does_not_count_as_a_change() {
        let was = vec![agent("t1", Status::Idle)];
        let mut now = was.clone();
        now[0].state_change_seq = 99;
        assert!(!rendered_differs(&was, &now));
    }

    #[test]
    fn a_status_change_counts() {
        let was = vec![agent("t1", Status::Idle)];
        let now = vec![agent("t1", Status::Working)];
        assert!(rendered_differs(&was, &now));
    }
```

- [ ] **Step 7: Run the tests**

Run: `cargo nextest run -p alacritree herdr::`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add alacritree/src/herdr.rs
git commit -m "feat(herdr): poll endpoints with two-tier backoff"
```

---

### Task 6: Match agents to workspaces

**Files:**
- Modify: `alacritree/src/herdr.rs`

**Interfaces:**
- Consumes: `Agent`, `Side` (Task 1), `wsl::linux_to_windows`.
- Produces: `herdr::match_workspace(agent: &Agent, side: &Side, workspaces: &[PathBuf]) -> Option<PathBuf>`.

- [ ] **Step 1: Write the failing test**

```rust
    use std::path::PathBuf;

    fn at(cwd: &str, foreground: Option<&str>) -> Agent {
        Agent {
            terminal_id: "t1".into(),
            pane_id: "w1:p1".into(),
            kind: None,
            status: Status::Idle,
            cwd: Some(cwd.into()),
            foreground_cwd: foreground.map(str::to_string),
            state_change_seq: 0,
        }
    }

    #[test]
    fn prefers_foreground_cwd_when_present() {
        let spaces = vec![PathBuf::from("/a"), PathBuf::from("/b")];
        let matched = match_workspace(&at("/a", Some("/b")), &Side::Native, &spaces);
        assert_eq!(matched, Some(PathBuf::from("/b")));
    }

    #[test]
    fn falls_back_to_cwd_when_foreground_is_absent() {
        let spaces = vec![PathBuf::from("/a")];
        assert_eq!(match_workspace(&at("/a/src", None), &Side::Native, &spaces), Some("/a".into()));
    }

    #[test]
    fn takes_the_longest_matching_prefix() {
        let spaces = vec![PathBuf::from("/a"), PathBuf::from("/a/nested")];
        let matched = match_workspace(&at("/a/nested/src", None), &Side::Native, &spaces);
        assert_eq!(matched, Some(PathBuf::from("/a/nested")));
    }

    /// Component-wise, so a sibling sharing a string prefix never matches.
    #[test]
    fn a_sibling_with_a_shared_prefix_does_not_match() {
        let spaces = vec![PathBuf::from("/repo")];
        assert_eq!(match_workspace(&at("/repo-other", None), &Side::Native, &spaces), None);
    }

    #[test]
    fn an_unmatched_agent_has_no_workspace() {
        let spaces = vec![PathBuf::from("/a")];
        assert_eq!(match_workspace(&at("/elsewhere", None), &Side::Native, &spaces), None);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p alacritree herdr::`
Expected: compile error, `cannot find function match_workspace`.

- [ ] **Step 3: Write minimal implementation**

```rust
use std::path::{Path, PathBuf};

/// The sidebar workspace an agent is working in, by longest path prefix.
/// `None` means it belongs under Home.
pub fn match_workspace(agent: &Agent, side: &Side, workspaces: &[PathBuf]) -> Option<PathBuf> {
    let reported = agent.foreground_cwd.as_deref().or(agent.cwd.as_deref())?;
    let cwd = match side {
        Side::Native => PathBuf::from(reported),
        Side::Wsl(distro) => crate::wsl::linux_to_windows(reported, distro),
    };
    workspaces
        .iter()
        .filter(|ws| starts_with(&cwd, ws))
        .max_by_key(|ws| ws.components().count())
        .cloned()
}

/// Component-wise prefix test.  Case-insensitive on Windows, where herdr
/// reports the cwd as the shell spelled it and `Path::starts_with` would
/// refuse `c:\users\lev` against `C:\Users\Lev`.
fn starts_with(cwd: &Path, workspace: &Path) -> bool {
    if cfg!(windows) {
        let mut want = workspace.components();
        let mut have = cwd.components();
        loop {
            match (want.next(), have.next()) {
                (None, _) => return true,
                (Some(_), None) => return false,
                (Some(w), Some(h)) => {
                    let (w, h) = (w.as_os_str(), h.as_os_str());
                    if !w.eq_ignore_ascii_case(h) {
                        return false;
                    }
                },
            }
        }
    } else {
        cwd.starts_with(workspace)
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p alacritree herdr::`
Expected: PASS.

- [ ] **Step 5: Add the Windows case test**

```rust
    #[cfg(windows)]
    #[test]
    fn windows_prefixes_compare_case_insensitively() {
        let spaces = vec![PathBuf::from(r"C:\Users\Lev\repo")];
        let matched = match_workspace(&at(r"c:\users\lev\repo\src", None), &Side::Native, &spaces);
        assert_eq!(matched, Some(PathBuf::from(r"C:\Users\Lev\repo")));
    }
```

- [ ] **Step 6: Run the tests**

Run: `cargo nextest run -p alacritree herdr::`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add alacritree/src/herdr.rs
git commit -m "feat(herdr): match agents to sidebar workspaces"
```

---

### Task 7: The sidebar row and the reconcile input

**Files:**
- Modify: `alacritree/src/sidebar_nav.rs`
- Modify: `alacritree/src/sidebar_focus.rs`
- Modify: `alacritree/src/steady_state.rs`
- Modify: `alacritree/src/app.rs`

**Interfaces:**
- Consumes: `herdr::Side` (Task 1), `EndpointCache::generation` (Task 5).
- Produces: `SidebarRow::HerdrAgent(Side, String)`, `sidebar_nav::ListedAgents`, `visible_rows(projects, sessions, agents)`, `UiInputs.herdr_generation`.

`visible_rows` runs only on a rebuild; the per-frame path is `ObservedInputs::matches`. So herdr state reaches the reconcile as one `u64`, not as a walked list.

- [ ] **Step 1: Write the failing test**

In `sidebar_nav.rs`'s test module:

```rust
#[test]
fn herdr_rows_follow_a_workspaces_own_sessions() {
    let projects = vec![project_with_worktree("/p", "/p/wt")];
    let mut sessions = ListedSessions::new();
    sessions.insert(Some(PathBuf::from("/p/wt")), vec![SessionId(1)]);
    let mut agents = ListedAgents::new();
    agents.insert(
        Some(PathBuf::from("/p/wt")),
        vec![(Side::Native, "term_a".to_string())],
    );

    let rows = visible_rows(&projects, &sessions, &agents);

    let wt = rows.iter().position(|r| matches!(r, SidebarRow::Worktree(_))).unwrap();
    assert!(matches!(rows[wt + 1], SidebarRow::Session(_)));
    assert!(matches!(rows[wt + 2], SidebarRow::HerdrAgent(..)));
}

#[test]
fn unmatched_agents_land_under_home() {
    let mut agents = ListedAgents::new();
    agents.insert(None, vec![(Side::Native, "term_a".to_string())]);
    let rows = visible_rows(&[], &ListedSessions::new(), &agents);
    assert!(matches!(rows[0], SidebarRow::Home));
    assert!(matches!(rows[1], SidebarRow::HerdrAgent(..)));
}
```

Use whichever project-construction helper the neighbouring `sidebar_nav` tests already use in place of `project_with_worktree`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p alacritree sidebar_nav::`
Expected: compile error, no variant `HerdrAgent`.

- [ ] **Step 3: Write minimal implementation**

In `sidebar_nav.rs`, add the variant to `SidebarRow`:

```rust
    /// A herdr-managed agent, keyed by the server it lives on and the
    /// terminal it runs in.  Both parts are needed: terminal ids are unique
    /// only within one server.
    HerdrAgent(Side, String),
```

Add the listing type and thread it through:

```rust
/// The herdr agent rows each workspace displays, keyed by workspace.  The
/// caller owns the listing rule, so the cursor model cannot drift from the
/// paint pass.
pub type ListedAgents = HashMap<WorkspaceKey, Vec<(Side, String)>>;

fn push_agent_rows(rows: &mut Vec<SidebarRow>, agents: &ListedAgents, ws: &WorkspaceKey) {
    if let Some(keys) = agents.get(ws) {
        rows.extend(keys.iter().map(|(side, id)| SidebarRow::HerdrAgent(side.clone(), id.clone())));
    }
}

pub fn visible_rows(
    projects: &[Project],
    sessions: &ListedSessions,
    agents: &ListedAgents,
) -> Vec<SidebarRow> {
    let mut rows = vec![SidebarRow::Home];
    push_session_rows(&mut rows, sessions, &None);
    push_agent_rows(&mut rows, agents, &None);
    for p in projects {
        rows.push(SidebarRow::Project(p.root.clone()));
        if p.expanded {
            for wt in &p.worktrees {
                rows.push(SidebarRow::Worktree(wt.path.clone()));
                push_session_rows(&mut rows, sessions, &Some(wt.path.clone()));
                push_agent_rows(&mut rows, agents, &Some(wt.path.clone()));
            }
        }
    }
    rows
}
```

Update every `visible_rows` caller to pass the new argument.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p alacritree sidebar_nav::`
Expected: PASS.

- [ ] **Step 5: Add the generation field to the reconcile**

In `sidebar_focus.rs`, add to `UiInputs` beside `pr_generation`:

```rust
    /// Advances when a herdr poll changes something a row draws.  Agent
    /// `revision` churn deliberately does not move it, so an idle agent
    /// repainting does not rebuild the tree.
    pub herdr_generation: u64,
```

Add `herdr_generation: u64` to `ObservedInputs`, set it in `capture`, and add it to the early-return comparison in `matches`:

```rust
            || self.pr_generation != ui.pr_generation
            || self.herdr_generation != ui.herdr_generation
```

Update every `UiInputs` literal in `sidebar_focus.rs`'s own tests and in `steady_state.rs` with `herdr_generation: 0`.

- [ ] **Step 6: Add the reconcile test**

In `sidebar_focus.rs`'s test module:

```rust
#[test]
fn a_herdr_generation_bump_invalidates_the_snapshot() {
    let inputs = ObservedInputs::capture(&[], std::iter::empty(), ui_inputs());
    let mut moved = ui_inputs();
    moved.herdr_generation = 1;
    assert!(inputs.matches(&[], std::iter::empty(), ui_inputs()));
    assert!(!inputs.matches(&[], std::iter::empty(), moved));
}
```

Use whichever helper the neighbouring tests use to build a default `UiInputs` in place of `ui_inputs()`.

- [ ] **Step 7: Run the tests**

Run: `cargo nextest run -p alacritree`
Expected: PASS, including `steady_state`.

- [ ] **Step 8: Commit**

```bash
git add alacritree/src/sidebar_nav.rs alacritree/src/sidebar_focus.rs \
        alacritree/src/steady_state.rs alacritree/src/app.rs
git commit -m "feat(sidebar): add herdr agent rows to the tree"
```

---

### Task 8: One agent, one row

**Files:**
- Modify: `alacritree/src/session.rs`
- Modify: `alacritree/src/herdr.rs`
- Modify: `alacritree/src/app.rs`

**Interfaces:**
- Consumes: `Side` (Task 1), `Agent` (Task 1), `ListedAgents` (Task 7).
- Produces: `herdr::HerdrKey`, `Session::herdr_key: Option<HerdrKey>`, `herdr::unattached(agents, claimed) -> Vec<&Agent>`.

The key lives on the `Session` so it dies with it. A side map would need clearing on every path that ends a session, and `reap_exited_sessions` is only one of them.

- [ ] **Step 1: Write the failing test**

In `herdr.rs`'s test module:

```rust
    #[test]
    fn an_attached_agent_yields_no_row() {
        let agents = vec![agent("t1", Status::Idle), agent("t2", Status::Working)];
        let claimed = [HerdrKey { side: Side::Native, terminal_id: "t1".into() }];
        let rows = unattached(&agents, &Side::Native, &claimed);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].terminal_id, "t2");
    }

    #[test]
    fn detaching_brings_the_row_back() {
        let agents = vec![agent("t1", Status::Idle)];
        assert_eq!(unattached(&agents, &Side::Native, &[]).len(), 1);
    }

    /// Terminal ids are unique only within one server, so a claim on one side
    /// must not hide the same id on another.
    #[test]
    fn a_claim_on_one_side_does_not_hide_the_other_side() {
        let agents = vec![agent("t1", Status::Idle)];
        let claimed = [HerdrKey { side: Side::Wsl("d".into()), terminal_id: "t1".into() }];
        assert_eq!(unattached(&agents, &Side::Native, &claimed).len(), 1);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p alacritree herdr::`
Expected: compile error, `cannot find type HerdrKey`.

- [ ] **Step 3: Write minimal implementation**

In `herdr.rs`:

```rust
/// Identifies one herdr agent across polls.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HerdrKey {
    pub side: Side,
    pub terminal_id: String,
}

/// The agents on `side` that no live session is attached to.  These are the
/// ones that get a sidebar row; an attached agent is drawn by its session
/// row instead, so each agent appears exactly once.
pub fn unattached<'a>(agents: &'a [Agent], side: &Side, claimed: &[HerdrKey]) -> Vec<&'a Agent> {
    agents
        .iter()
        .filter(|a| {
            !claimed
                .iter()
                .any(|k| k.side == *side && k.terminal_id == a.terminal_id)
        })
        .collect()
}
```

In `session.rs`, add the field to `Session`:

```rust
    /// Set when this session is a shell attached to a herdr agent, so the
    /// sidebar draws one row for that agent rather than two.  Dies with the
    /// session, which is why it lives here and not in a map.
    pub herdr_key: Option<herdr::HerdrKey>,
```

Initialise it to `None` in every `Session` constructor.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p alacritree herdr::`
Expected: PASS.

- [ ] **Step 5: Build the listing in app.rs**

Add to `AlacritreeApp`, and call it from the same place `ListedSessions` is assembled for the sidebar:

```rust
    /// Rows for every herdr agent no session is attached to, bucketed by the
    /// workspace it is working in.  Unmatched agents land under Home, which
    /// is the common case: an agent in a repository alacritree does not
    /// track still belongs somewhere.
    fn listed_herdr_agents(&self) -> sidebar_nav::ListedAgents {
        let mut listed = sidebar_nav::ListedAgents::new();
        if !self.config.ui.herdr.enabled {
            return listed;
        }
        let claimed: Vec<herdr::HerdrKey> =
            self.sessions.iter().filter_map(|s| s.herdr_key.clone()).collect();
        let workspaces: Vec<PathBuf> = self
            .projects
            .iter()
            .flat_map(|p| p.worktrees.iter().map(|wt| wt.path.clone()))
            .collect();

        for cache in &self.herdr_endpoints {
            let side = cache.side();
            for agent in herdr::unattached(cache.agents(), side, &claimed) {
                let workspace = herdr::match_workspace(agent, side, &workspaces);
                if workspace.is_none() && !self.config.ui.herdr.show_unmatched {
                    continue;
                }
                listed
                    .entry(workspace)
                    .or_default()
                    .push((side.clone(), agent.terminal_id.clone()));
            }
        }
        listed
    }

    /// One number standing for every endpoint's rendered state, so the
    /// sidebar's per-frame comparison stays a `u64` compare.
    fn herdr_generation(&self) -> u64 {
        self.herdr_endpoints.iter().map(herdr::EndpointCache::generation).sum()
    }
```

Add `herdr_endpoints: Vec<herdr::EndpointCache>` to `AlacritreeApp`, seeded with `Side::Native` plus one `Side::Wsl(distro)` per distro whose helper reports a herdr path through `wsl_helper::capability_herdr`. Add `EndpointCache::side(&self) -> &Side` to `herdr.rs`. In `update`, call `cache.poll(self.config.ui.herdr.poll_interval)` for each endpoint, and pass `self.herdr_generation()` as the `herdr_generation` field of `UiInputs`.

- [ ] **Step 6: Run the tests**

Run: `cargo nextest run -p alacritree`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add alacritree/src/herdr.rs alacritree/src/session.rs alacritree/src/app.rs
git commit -m "feat(herdr): draw one row per agent, attached or not"
```

---

### Task 9: Attach, and keep a failed attach visible

**Files:**
- Modify: `alacritree/src/app.rs`
- Modify: `alacritree/src/herdr.rs`

**Interfaces:**
- Consumes: `attach_args`, `session_attach_script` (Task 2), `HerdrKey` (Task 8), `spawn_session_with_shell` (`app.rs:1302` on the base), `reap_exited_sessions` (`app.rs:7308` on the base).
- Produces: `AlacritreeApp::attach_herdr_agent(&mut self, ctx, key, pane_id, workspace)`, `herdr::can_attach(side: &Side) -> bool`.

An attach refused for any reason exits within a frame. Reaping it silently makes the pane flash and vanish, so a session holding a herdr key survives a non-zero exit.

- [ ] **Step 1: Write the failing test**

In `herdr.rs`'s test module:

```rust
    #[test]
    fn native_windows_cannot_attach_directly() {
        assert_eq!(can_attach(&Side::Native), !cfg!(windows));
    }

    /// A WSL server runs herdr's unix build whatever the host is.
    #[test]
    fn wsl_can_always_attach() {
        assert!(can_attach(&Side::Wsl("d".into())));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p alacritree herdr::`
Expected: compile error, `cannot find function can_attach`.

- [ ] **Step 3: Write minimal implementation**

```rust
/// Whether direct per-agent attach works on this side.  herdr's
/// `run_terminal_attach` is a `#[cfg(windows)]` refusal, so a native Windows
/// server falls back to focusing the pane and attaching the whole session.
pub fn can_attach(side: &Side) -> bool {
    match side {
        Side::Native => !cfg!(windows),
        Side::Wsl(_) => true,
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p alacritree herdr::`
Expected: PASS.

- [ ] **Step 5: Wire the attach action**

In `app.rs`, add:

```rust
    /// Opens a herdr agent in a session running herdr's attach client.  The
    /// session is an ordinary shell, so nothing in the grid or input path
    /// treats it specially; only the key marks it as this agent's row.
    fn attach_herdr_agent(
        &mut self,
        ctx: &Context,
        key: herdr::HerdrKey,
        pane_id: &str,
        workspace: WorkspaceKey,
    ) {
        let args = if herdr::can_attach(&key.side) {
            herdr::attach_args(pane_id)
        } else {
            let session = herdr::running_session_name(&key.side);
            vec!["-c".into(), herdr::session_attach_script(pane_id, &session)]
        };
        let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
        let (program, argv) = key.side.command(&borrowed);
        // `alacritty_terminal::tty::Shell`'s fields are crate-private, so
        // this goes through the constructor rather than a struct literal.
        let shell = Shell::new(program, argv);
        match self.spawn_session_with_shell(ctx, workspace, Some(shell), None) {
            Ok(id) => {
                if let Some(session) = self.sessions.iter_mut().find(|s| s.id == id) {
                    session.herdr_key = Some(key);
                }
            },
            Err(e) => self.error_dialog = Some(format!("failed to attach herdr agent: {e}")),
        }
    }
```

Add the session-name lookup to `herdr.rs`, since the fallback path needs a name and must not assume one:

```rust
#[derive(Deserialize)]
struct SessionList {
    #[serde(default)]
    sessions: Vec<RawSession>,
}

#[derive(Deserialize)]
struct RawSession {
    name: String,
    #[serde(default)]
    running: bool,
}

/// The running session to attach to on this side.  `herdr session list
/// --json` is a flat object rather than the `result`-wrapped envelope
/// `agent list` uses.  Falls back to `default`, which is the name herdr
/// gives an unnamed session.
pub fn running_session_name(side: &Side) -> String {
    let (program, args) = side.command(&["session", "list", "--json"]);
    let fallback = || "default".to_string();
    let Ok(output) = Command::new(program)
        .hide_console()
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
    else {
        return fallback();
    };
    if !output.status.success() {
        return fallback();
    }
    serde_json::from_slice::<SessionList>(&output.stdout)
        .ok()
        .and_then(|list| list.sessions.into_iter().find(|s| s.running).map(|s| s.name))
        .unwrap_or_else(fallback)
}
```

It spawns a subprocess, so it is called only from the attach action, which already runs on a user gesture rather than in `update`'s hot path.

Note the two existing side effects of `spawn_session_with_shell`: it refuses when `worktree_gone(dir)` and runs `sync_doppler_scopes(dir)` first. Neither needs changing; the refusal surfaces through the `Err` arm above.

- [ ] **Step 6: Keep a failed attach on screen**

Change `reap_exited_sessions` (`app.rs:7308` on the base) so a session holding a herdr key is reaped only on a clean exit:

```rust
    fn reap_exited_sessions(&mut self, ctx: &Context) {
        let exited_ids: Vec<SessionId> = self
            .sessions
            .iter()
            .filter(|s| s.is_exited())
            // A refused attach — the agent already has a client, or herdr
            // refuses on this platform — exits within a frame.  Reaping it
            // would take herdr's message with it and leave only a flash, so
            // the shell stays until the user closes it.
            .filter(|s| s.herdr_key.is_none() || s.exit_was_clean())
            .map(|s| s.id)
            .collect();
        for id in exited_ids {
            self.close_session(ctx, id);
        }
    }
```

Add `Session::exit_was_clean(&self) -> bool` in `session.rs`, backed by the `ExitStatus` already carried on `TermEvent::ChildExit` (`session.rs:1662` on the base), which currently discards it.

- [ ] **Step 7: Run the tests**

Run: `cargo nextest run -p alacritree`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add alacritree/src/app.rs alacritree/src/herdr.rs alacritree/src/session.rs
git commit -m "feat(herdr): attach agents and keep failures readable"
```

---

### Task 10: Paint the rows

**Files:**
- Modify: `alacritree/src/app.rs`
- Modify: `alacritree/src/row_label.rs`

**Interfaces:**
- Consumes: everything above.
- Produces: no new public API; this task renders `SidebarRow::HerdrAgent` and binds Enter to `attach_herdr_agent`.

- [ ] **Step 1: Write the failing test**

In `row_label.rs`'s test module:

```rust
#[test]
fn an_unknown_agent_still_reads_as_an_agent() {
    assert_eq!(herdr_glyph(Some("claude")), herdr_glyph(Some("claude")));
    assert!(herdr_glyph(Some("qwen")).is_some());
    assert!(herdr_glyph(None).is_some());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p alacritree row_label::`
Expected: compile error, `cannot find function herdr_glyph`.

- [ ] **Step 3: Write minimal implementation**

```rust
/// Glyph for a herdr-managed agent.  alacritree recognises six agent names
/// (`AGENT_PROCESS_NAMES`) while herdr detects many more, so an unfamiliar
/// name falls back to a generic agent glyph rather than to nothing.
pub fn herdr_glyph(kind: Option<&str>) -> Option<&'static str> {
    Some(match kind.map(str::to_ascii_lowercase).as_deref() {
        Some(name) if name.starts_with("claude") => GLYPH_CLAUDE,
        Some(name) if name.starts_with("codex") => GLYPH_CODEX,
        _ => GLYPH_AGENT_GENERIC,
    })
}
```

Reuse whichever glyph constants `session.rs`'s existing agent rendering already defines, and add `GLYPH_AGENT_GENERIC` beside them if none exists.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p alacritree row_label::`
Expected: PASS.

- [ ] **Step 5: Render the row**

Mirror `session_row` (`app.rs:7205` on the base), which is the free function the sidebar already calls per session row and returns a `SessionRowAction`. Add its herdr twin beside it:

```rust
struct HerdrRowAction {
    attach: bool,
}

/// A herdr agent nothing is attached to.  Drawn in `theme.text_dim` because
/// it is listed but not live — the same weight `worktree_gone` gives a row
/// whose checkout has been removed.  An attached agent has an ordinary
/// session row instead, so no agent is ever drawn twice.
fn herdr_row(
    ui: &mut egui::Ui,
    row: &HerdrRowData,
    is_cursor: bool,
    scroll_into_view: bool,
    icons: &Icons,
    theme: &Theme,
) -> HerdrRowAction
```

`HerdrRowData` carries what the row draws: `glyph: &'static str` from `herdr_glyph(kind)`, `name: String` (the agent kind, falling back to the last six characters of the terminal id), `status: herdr::Status`, and `shared_view: bool` set from `!herdr::can_attach(side)`.

Copy `session_row`'s body as the starting point — the `Shape::Noop` background slot, the cursor and hover handling, the `scroll_into_view` call — and change what it paints: every label in `theme.text_dim`, the status word after the name, a `herdr` marker, and `shared view` appended when `shared_view` is set. Drop the close button; a herdr row has nothing to close.

In the sidebar's row loop, add the `SidebarRow::HerdrAgent(side, terminal_id)` arm that looks the agent up in the endpoint caches, builds `HerdrRowData`, calls `herdr_row`, and on `attach` calls `attach_herdr_agent` with the row's `HerdrKey`, the agent's `pane_id`, and the workspace the row sits under. Bind Enter the same way the session arm binds it.

Give the row a tooltip naming `Ctrl+B q` as herdr's detach key, since nothing else in alacritree uses that chord and the user has no other way to learn it.

- [ ] **Step 6: Verify by running the app**

Run: `cargo run -p alacritree`
With a herdr server running and an agent in it, confirm: the agent appears dimmed under its worktree, Enter attaches, the row is replaced by a session row at full strength, detaching with `Ctrl+B q` restores the dimmed row, and stopping the herdr server empties the list without stalling the UI.

- [ ] **Step 7: Run the full suite and format**

Run: `cargo fmt && cargo nextest run -p alacritree`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add alacritree/src/app.rs alacritree/src/row_label.rs
git commit -m "feat(sidebar): render and attach herdr agent rows"
```

---

## Verification

Before opening the PR:

```sh
cargo fmt --check
cargo clippy -p alacritree --all-targets
cargo nextest run -p alacritree
cargo test -p alacritree --test config_schema
```

Manual checks that no test covers, because none may invoke a real herdr binary:

1. **No herdr installed.** Rename the binary; alacritree starts with no herdr rows, no error dialog, and one failed spawn in the log rather than one every two seconds.
2. **Server restarted under a running alacritree.** `herdr server stop` then `herdr`; rows disappear and come back within ~30 s without restarting alacritree.
3. **Second attach refused.** Attach the same agent twice; the second session stays open showing herdr's refusal instead of flashing.
4. **Native Windows fallback.** Confirm the shared-view marker renders, and that attaching resizes the herdr window as the spec says it will.
