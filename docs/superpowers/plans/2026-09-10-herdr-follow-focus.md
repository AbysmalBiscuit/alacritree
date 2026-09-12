# Following herdr's focus from any session: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a focus change inside herdr move alacritree even when the active session is a plain native one, behind a tri-state config gate whose default is today's behavior, and give `ListSessions` the vocabulary an end-to-end test needs to assert where alacritree landed.

**Architecture:** `HerdrViewSync::next` keeps one entry point and gains a private per-side focus trail. The shared-view path runs first and is untouched. When it yields nothing, the mode is `"always"` and the user is attentive, the trail compares each endpoint cache's currently focused terminal against what it recorded and turns the first difference into a pending follow, which waits for a gap in direct input and expires if that gap never comes. The `ListSessions` reply grows `agent`, `busy` and `multiplexer` so an out-of-process test can name the pane alacritree moved to.

**Tech Stack:** Rust 2024 (workspace MSRV 1.85), egui/eframe, `serde_json`, `schemars` for the published config schema, `windows-sys` for the two Win32 calls the end-to-end suite makes, nextest as the runner.

**Spec:** `docs/superpowers/specs/2026-09-09-herdr-follow-focus-design.md`. Its adversarial review, with a `file:line` citation for every claim, is `docs/superpowers/specs/2026-09-09-herdr-follow-focus-review.md`.

## Global Constraints

- The default `follow_focus = "herdr"` must leave today's behavior unchanged. An unmodified config sees no new movement.
- The `Focus` arm runs whatever the setting says. The setting governs whether herdr may move alacritree, never whether alacritree may move herdr.
- Build, test, lint and format through devkit, never by typing cargo: `devkit run task fmt`, `devkit run task check`, `devkit run task test`, `devkit run task clippy`. `fmt` is nightly rustfmt; `test` is nextest.
- The schema is regenerated, never hand-edited. `AGENTS.md` prescribes `ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema`, which this checkout's tooling hook refuses; `alacritree/tests/config_schema.rs` documents `cargo run -p alacritree -- schema > schema/alacritree-config.json` as the equivalent, and the `config_schema` test is the guard either way.
- Never disable the rustc cache with an environment-variable prefix. Caching works correctly on this system.
- Several agents share this checkout. Claim every file before editing it: `DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm acquire <abs path> --note "<why>"`. Release when the edit and its verification are done.
- Never use `git stash`. To read another revision, use `git show REV:path`.
- Write absolute paths into commands rather than changing directory first.
- Comments explain the *why*, never the *what*. They are timeless and standalone: no PR or task references, no `now we` / `used to` / `this PR`, no RED/GREEN narration.
- Conventional Commits. Imperative subject under 72 characters, lowercase after the colon, no trailing period. Body wrapped at 72. Every commit ends with `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.
- Stage selectively. Review `git diff --staged` before committing.
- Do not open or push a PR. That is the user's call.
- New UX behavior needs a config option to enable it, so Arnaud's workflow is unaffected.
- The timing constants are `750 ms` for the quiet gap and `10 s` for the expiry. Both are `const`, not config keys.

## File structure

| File | Responsibility |
|---|---|
| `alacritree/src/config.rs` | `FollowFocus` enum, `parse_follow_focus`, the `HerdrConfig` field, the `RawHerdr` field with its schema default and enum, and the resolution wiring. |
| `schema/alacritree-config.json` | Regenerated, never hand-edited. |
| `alacritree/src/herdr/view.rs` | `ViewInputs`, the restructured `next`, the focus trail, the pending follow, and every unit test for them. |
| `alacritree/src/app.rs` | The `last_direct_input` clock, the new `next` call site, trail stamping at the three focus-moving paths, and the `ListSessions` reply fields. |
| `alacritree/src/cli/render.rs` | Human-readable rendering of the new reply fields. |
| `alacritree/tests/herdr_e2e.rs` | The opt-in end-to-end suite that pilots the real binary against a throwaway herdr server. |
| `alacritree/Cargo.toml` | The Win32 features the end-to-end suite needs as a Windows dev-dependency. |
| `devkit.local.toml` (main checkout) | The `e2e` task. |

---

### Task 1: The config option

**Files:**
- Modify: `alacritree/src/config.rs` (around `560-610`, `2572-2605`, `3232-3246`)
- Modify: `schema/alacritree-config.json` (regenerated)
- Test: `alacritree/src/config.rs`, in the existing `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `pub enum FollowFocus { Off, Herdr, Always }` with `#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]` and `#[default] Herdr`; the field `HerdrConfig::follow_focus: FollowFocus`. Every later task reads `config.integrations.herdr.follow_focus`.

- [ ] **Step 1: Claim the file**

```sh
DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm acquire /c/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/config.rs --note "herdr follow_focus config option"
```

- [ ] **Step 2: Write the failing tests**

Add to the existing `mod tests` in `alacritree/src/config.rs`:

```rust
#[test]
fn herdr_follow_focus_defaults_to_the_shipped_behavior() {
    let config = Config::default();
    assert_eq!(config.integrations.herdr.follow_focus, FollowFocus::Herdr);
}

#[test]
fn herdr_follow_focus_reads_each_accepted_word() {
    for (word, expected) in [
        ("off", FollowFocus::Off),
        ("herdr", FollowFocus::Herdr),
        ("always", FollowFocus::Always),
    ] {
        assert_eq!(parse_follow_focus(Some(word)), expected);
    }
}

#[test]
fn herdr_follow_focus_falls_back_on_an_unknown_word() {
    assert_eq!(parse_follow_focus(Some("sideways")), FollowFocus::Herdr);
    assert_eq!(parse_follow_focus(None), FollowFocus::Herdr);
}
```

Find the existing `show_panes = true` TOML test near line 3536 and add a sibling that parses the key end to end:

```rust
#[test]
fn herdr_follow_focus_parses_from_toml() {
    let config = Config::from_str(
        r#"
[integrations.herdr]
follow_focus = "always"
"#,
    );
    assert_eq!(config.integrations.herdr.follow_focus, FollowFocus::Always);
}
```

If the neighbouring TOML test uses a different constructor than `Config::from_str`, copy whichever one it uses verbatim rather than inventing a new entry point.

- [ ] **Step 3: Run the tests to verify they fail**

Run: `devkit run task test`
Expected: FAIL, `cannot find type FollowFocus in this scope`.

- [ ] **Step 4: Add the enum and the parser**

Directly below `parse_attach_mode` in `alacritree/src/config.rs`:

```rust
/// Whether a focus change made inside herdr may move alacritree, and from
/// which sessions.  Following moves the keyboard, so the default is the
/// narrower rule: only a session that is already showing herdr's view
/// follows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub enum FollowFocus {
    /// herdr never moves alacritree.  alacritree still tells herdr where to
    /// point when the user picks a row.
    Off,
    /// Follow only while the active session is herdr-backed.
    #[default]
    Herdr,
    /// Follow from a native session too, on any reachable side.
    Always,
}

fn parse_follow_focus(raw: Option<&str>) -> FollowFocus {
    match raw {
        None => FollowFocus::default(),
        Some("off") => FollowFocus::Off,
        Some("herdr") => FollowFocus::Herdr,
        Some("always") => FollowFocus::Always,
        Some(other) => {
            log::warn!("unknown integrations.herdr.follow_focus value {other:?}, using \"herdr\"");
            FollowFocus::default()
        },
    }
}
```

- [ ] **Step 5: Add the resolved field**

In `pub struct HerdrConfig`, after `attach`:

```rust
    /// Whether a focus change inside herdr moves alacritree, and from which
    /// sessions.
    pub follow_focus: FollowFocus,
```

In `impl Default for HerdrConfig`, after `attach: AttachMode::default(),`:

```rust
            follow_focus: FollowFocus::default(),
```

- [ ] **Step 6: Add the raw field**

In `struct RawHerdr`, after the `attach` field. Unlike `attach`, this one is a bare `String` under the type's `Default`, so the resolved value and the published schema default share one source, which is what `AGENTS.md` asks of every key whose omission resolves to a fixed value:

```rust
    /// Whether a focus change made inside herdr moves alacritree to the
    /// matching session.
    ///
    /// "off" never moves it.  "herdr" (default) moves it only while the
    /// active session is already showing herdr's view, which is what an
    /// unmodified config has always done.  "always" additionally moves it
    /// from a plain native session, on any reachable side, after a gap in
    /// typing.
    #[schemars(extend("enum" = ["off", "herdr", "always"]))]
    follow_focus: String,
```

`RawHerdr` derives `Default`, and a bare `String` defaults to the empty string, which `parse_follow_focus` would warn about. Give the type a hand-written `Default` instead of the derive, so the default is `"herdr"`:

```rust
impl Default for RawHerdr {
    fn default() -> Self {
        Self {
            enabled: None,
            poll_interval_ms: None,
            show_unmatched: None,
            show_panes: None,
            attach: None,
            follow_focus: "herdr".to_string(),
        }
    }
}
```

Remove `Default` from `RawHerdr`'s `#[derive(...)]` list when you add this, leaving `#[derive(Debug, Deserialize, JsonSchema)]` and the `#[serde(default)]` attribute in place.

- [ ] **Step 7: Wire the resolution**

In the `HerdrConfig` literal near line 3241, after the `attach` line:

```rust
                    follow_focus: parse_follow_focus(Some(
                        self.integrations.herdr.follow_focus.as_str(),
                    )),
```

- [ ] **Step 8: Run the tests to verify they pass**

Run: `devkit run task test`
Expected: PASS, except `config_schema`, which now fails because the published schema is stale.

- [ ] **Step 9: Regenerate the schema**

Run: `cargo run -p alacritree -- schema > schema/alacritree-config.json`

Then confirm `schema/alacritree-config.json` gained a `follow_focus` property carrying `"default": "herdr"` and the three-word `enum`. `schema_defaults` and its allowlist live on the stacked config-defaults branch and are not on this one, so do not go looking for them.

- [ ] **Step 10: Format, lint and run the whole suite**

```sh
devkit run task fmt
devkit run task clippy
devkit run task test
```

Expected: clean, all green.

- [ ] **Step 11: Commit**

```sh
git add alacritree/src/config.rs schema/alacritree-config.json
git commit -m "$(cat <<'EOF'
feat(herdr): add the follow_focus config gate

Following a focus change made inside herdr moves the keyboard, so it
needs a switch before it can reach sessions that do not already show
herdr's view. A bool cannot express the three states this has: off,
today's behavior, and the new one. Defaulting to today's behavior
leaves an unmodified config untouched.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 12: Release the claim**

```sh
DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm release /c/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/config.rs
```

---

### Task 2: What ListSessions reports

**Files:**
- Modify: `alacritree/src/app.rs` (`session_json` at `11494`, the `ListSessions` arm at `11343`)
- Modify: `alacritree/src/cli/render.rs` (`sessions` at `70`)
- Test: `alacritree/src/app.rs` in the existing `#[cfg(test)] mod tests`; `alacritree/src/cli/render.rs` in its own

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: three nullable fields on each entry of the `ListSessions` reply. Task 5 and Task 8 assert on `multiplexer.terminal_id` and `is_active_tab`.

The shape, for one herdr-backed session:

```json
{
  "id": 3,
  "title": "claude",
  "workspace": "...",
  "kind": "shell",
  "is_active_tab": true,
  "needs_attention": false,
  "agent": { "name": "claude", "state": "working" },
  "busy": null,
  "multiplexer": {
    "name": "herdr",
    "side": "native",
    "session": "default",
    "terminal_id": "w1:t2",
    "pane_id": "w1:p3",
    "tab_id": "w1:t2"
  }
}
```

- [ ] **Step 1: Claim the files**

```sh
DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm acquire \
  /c/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/app.rs \
  /c/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/cli/render.rs \
  --note "ListSessions activity and multiplexer fields"
```

- [ ] **Step 2: Write the failing tests**

In `alacritree/src/app.rs`'s `mod tests`. Match the surrounding tests' way of building an `AlacritreeApp`; if they use a helper such as `test_app()`, use it rather than constructing one by hand.

```rust
#[test]
fn a_plain_shell_session_reports_no_agent_and_no_multiplexer() {
    let app = test_app();
    let session = app.sessions.first().expect("the app starts with a session");
    let json = app.session_json(session, true);
    assert_eq!(json["agent"], Value::Null);
    assert_eq!(json["multiplexer"], Value::Null);
    assert!(json["busy"].is_boolean());
}

#[test]
fn a_herdr_backed_session_names_its_side_and_terminal() {
    let mut app = test_app();
    let key = herdr::HerdrKey { side: herdr::Side::Native, terminal_id: "t7".into() };
    let id = app.sessions.first().expect("a session").id;
    if let Some(session) = app.sessions.iter_mut().find(|s| s.id == id) {
        session.bind_herdr(key);
    }
    let session = app.sessions.iter().find(|s| s.id == id).expect("a session");
    let json = app.session_json(session, true);
    assert_eq!(json["multiplexer"]["name"], "herdr");
    assert_eq!(json["multiplexer"]["side"], "native");
    assert_eq!(json["multiplexer"]["terminal_id"], "t7");
    // The attach client is itself the foreground job, so probing it would
    // answer true forever.
    assert_eq!(json["busy"], Value::Null);
}

#[test]
fn a_wsl_side_spells_its_distro() {
    let mut app = test_app();
    let key =
        herdr::HerdrKey { side: herdr::Side::Wsl("ubuntu".into()), terminal_id: "t1".into() };
    let id = app.sessions.first().expect("a session").id;
    if let Some(session) = app.sessions.iter_mut().find(|s| s.id == id) {
        session.bind_herdr(key);
    }
    let session = app.sessions.iter().find(|s| s.id == id).expect("a session");
    assert_eq!(app.session_json(session, true)["multiplexer"]["side"], "wsl:ubuntu");
}
```

In `alacritree/src/cli/render.rs`'s `mod tests`, extend the existing `absent_fields_do_not_panic` coverage with one that exercises the new fields:

```rust
#[test]
fn a_session_line_names_its_agent_state_and_multiplexer() {
    human(
        &IpcRequest::ListSessions,
        &serde_json::json!({
            "sessions": [{
                "id": 1,
                "title": "claude",
                "workspace": "/repo",
                "is_active_tab": true,
                "needs_attention": false,
                "agent": { "name": "claude", "state": "working" },
                "busy": serde_json::Value::Null,
                "multiplexer": { "name": "herdr", "side": "native", "terminal_id": "t1" },
            }]
        }),
    );
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `devkit run task test`
Expected: FAIL, `no method named session_json found for struct AlacritreeApp`.

- [ ] **Step 4: Turn `session_json` into a method**

`session_json` is a free function today and the new fields reach past a `&Session`: `agent` folds herdr's status in through `herdr_backed_activity`, and `multiplexer` needs `find_herdr_agent` and `herdr_session_name`, both `AlacritreeApp` methods. Move it into the `impl AlacritreeApp` block that holds `find_herdr_agent` (near line `9588`) and delete the free function:

```rust
    /// One session as the IPC reply describes it.  `agent`, `busy` and
    /// `multiplexer` are nullable because a plain shell has no agent, a
    /// multiplexer-backed session has no foreground job of its own to probe,
    /// and a session owning its PTY belongs to no multiplexer.
    fn session_json(&self, session: &Session, is_active_tab: bool) -> Value {
        let key = session.herdr_key.as_ref();
        let agent = key
            .and_then(|key| self.find_herdr_agent(&key.side, &key.terminal_id))
            .and_then(|agent| agent.status);
        let activity = herdr_backed_activity(session.activity(), agent);
        json!({
            "id": session.id,
            "title": session.title,
            "workspace": session.working_directory,
            "kind": match &session.kind {
                SessionKind::Shell => "shell",
                SessionKind::Diff { .. } => "diff",
                SessionKind::Scratchpad { .. } => "scratchpad",
            },
            "columns": session.size.columns,
            "lines": session.size.screen_lines,
            "is_active_tab": is_active_tab,
            "needs_attention": session.needs_attention,
            "agent": activity_json(activity),
            "busy": key.is_none().then(|| session.is_busy()),
            "multiplexer": key.map(|key| self.multiplexer_json(key)),
        })
    }

    /// Where a herdr-backed session lives.  `pane_id` and `tab_id` come from
    /// the live listing, so a null says the cache does not know right now
    /// rather than that the pane is gone.
    fn multiplexer_json(&self, key: &herdr::HerdrKey) -> Value {
        let pane = self.find_herdr_agent(&key.side, &key.terminal_id);
        json!({
            "name": "herdr",
            "side": side_label(&key.side),
            "session": self.herdr_session_name(&key.side),
            "terminal_id": key.terminal_id,
            "pane_id": pane.map(|pane| pane.pane_id.clone()),
            "tab_id": pane.and_then(|pane| pane.tab_id.clone()),
        })
    }
```

Add the two free helpers beside the old `session_json` site:

```rust
/// `SessionActivity` as the reply spells it.  A plain shell is null rather
/// than an object, so a consumer testing for presence needs no second field.
fn activity_json(activity: SessionActivity) -> Value {
    match activity {
        SessionActivity::Shell => Value::Null,
        SessionActivity::Agent { name, live } => json!({
            "name": name,
            "state": live.label(),
        }),
    }
}

/// Two herdr servers on one machine cannot see each other, so the side is
/// part of an agent's identity and the reply spells it out.
fn side_label(side: &herdr::Side) -> String {
    match side {
        herdr::Side::Native => "native".to_string(),
        herdr::Side::Wsl(distro) => format!("wsl:{distro}"),
    }
}
```

- [ ] **Step 5: Fix the call site**

In the `Req::ListSessions` arm near line `11343`, `session_json(s, active)` becomes `self.session_json(s, active)`. The closure borrows `self` immutably while `self.sessions` is also borrowed immutably, which is fine; if the borrow checker objects because the surrounding function takes `&mut self`, collect the ids and active flags first:

```rust
            Req::ListSessions => {
                let rows: Vec<(SessionId, bool)> = self
                    .sessions
                    .iter()
                    .map(|s| {
                        let active =
                            self.active_session.get(&s.working_directory).copied() == Some(s.id);
                        (s.id, active)
                    })
                    .collect();
                let sessions: Vec<Value> = rows
                    .into_iter()
                    .filter_map(|(id, active)| {
                        let session = self.sessions.iter().find(|s| s.id == id)?;
                        Some(self.session_json(session, active))
                    })
                    .collect();
                Ok(json!({ "current_workspace": self.current_workspace, "sessions": sessions }))
            },
```

- [ ] **Step 6: Render the new fields**

In `alacritree/src/cli/render.rs`, replace the body of `sessions`'s loop so the agent state and the multiplexer show in the human-readable path. `--json` still carries everything:

```rust
    for s in sessions {
        // The active tab and an attention flag are the two things worth
        // scanning a list for; everything else is in --json.
        let active = if s["is_active_tab"].as_bool().unwrap_or(false) { "*" } else { " " };
        let attention = if s["needs_attention"].as_bool().unwrap_or(false) { " (!)" } else { "" };
        let workspace = s["workspace"].as_str().unwrap_or("home");
        let state = match s["agent"]["state"].as_str() {
            Some(state) => format!("  [{state}]"),
            None => String::new(),
        };
        let via = match s["multiplexer"]["name"].as_str() {
            Some(name) => format!("  via {name}"),
            None => String::new(),
        };
        println!(
            "{active} {}  {}  {workspace}{state}{via}{attention}",
            text(&s["id"]),
            text(&s["title"])
        );
    }
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `devkit run task test`
Expected: PASS.

- [ ] **Step 8: Format, lint**

```sh
devkit run task fmt
devkit run task clippy
```

- [ ] **Step 9: Commit**

```sh
git add alacritree/src/app.rs alacritree/src/cli/render.rs
git commit -m "$(cat <<'EOF'
feat(ipc): report session activity and multiplexer identity

A caller reading the session list could tell what a session was named
but not what runs in it or where it lives. Both answers already exist
for the sidebar and the command palette, so the reply grows three
nullable fields over the accessors that serve them.

Knowing whether a session is native or multiplexer-backed, and which
multiplexer, is the first thing a consumer asks, and it is what an
out-of-process test needs to name the pane it expects.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 10: Release the claims**

```sh
DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm release \
  /c/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/app.rs \
  /c/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/cli/render.rs
```

---

### Task 3: The end-to-end harness

**Files:**
- Create: `alacritree/tests/herdr_e2e.rs`
- Modify: `alacritree/Cargo.toml` (the `[target.'cfg(windows)'.dev-dependencies]` block at `157-160`)
- Modify: `devkit.local.toml` in the **main checkout** at `C:/Users/Lev/Git/github/alacritree/devkit.local.toml`

**Interfaces:**
- Consumes: the `multiplexer` and `is_active_tab` fields from Task 2.
- Produces: `struct Harness` with `fn start() -> Harness`, `fn herdr(&self, args: &[&str]) -> Output`, `fn alacritree(&self, args: &[&str]) -> Output`, `fn sessions(&self) -> serde_json::Value`, `fn foreground_is_child(&self) -> bool`, and a `Drop` that tears down in the fixed order. Tasks 5 and 8 add tests that use it.

This task delivers the harness plus the one test that passes today, which is what proves the harness works before anything depends on it failing.

- [ ] **Step 1: Claim the files**

```sh
DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm acquire \
  /c/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/Cargo.toml \
  --note "win32 dev-dependency features for the herdr e2e suite"
```

`alacritree/tests/herdr_e2e.rs` does not exist yet, so it needs no claim until it does. `devkit.local.toml` lives in the main checkout, outside this worktree, and `lockm` refuses a path outside the project root; edit it without a claim.

- [ ] **Step 2: Extend the Windows dev-dependency**

The suite calls `GetForegroundWindow` and `GetWindowThreadProcessId` from `Win32_UI_WindowsAndMessaging`, and `SendInput` from `Win32_UI_Input_KeyboardAndMouse`. In `alacritree/Cargo.toml`, replace the `[target.'cfg(windows)'.dev-dependencies]` `windows-sys` line:

```toml
[target.'cfg(windows)'.dev-dependencies]
# Share-mode constants for tests that hold a file the way the loader holds a
# running exe image: writes denied, rename allowed. The UI features are the
# foreground check and the synthetic input the end-to-end suite needs, which
# no CLI surface can produce.
windows-sys = { version = "0.59", features = [
    "Win32_Storage_FileSystem",
    "Win32_Foundation",
    "Win32_UI_WindowsAndMessaging",
    "Win32_UI_Input_KeyboardAndMouse",
] }
```

- [ ] **Step 3: Write the harness and the passing test**

Create `alacritree/tests/herdr_e2e.rs`:

```rust
//! Following herdr's focus, driven through the real binary.
//!
//! Every check here is about what the running window does when herdr's focus
//! moves, which no in-crate test can observe: following spawns attach
//! clients, switches workspace and takes terminal focus. So these pilot the
//! executable against a herdr server of their own.
//!
//! Opt-in. They need a herdr binary, they hold the foreground for their whole
//! run, and a window appears while they do.

#![cfg(windows)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

/// How long a piloted assertion waits for the window to catch up. The poll
/// interval puts up to two seconds of age on a focus change before alacritree
/// can see it at all, and an attach on Windows costs more.
const SETTLE: Duration = Duration::from_secs(15);

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_alacritree")
}

/// The session name is the whole isolation story: it names the herdr session,
/// the config directory under it, and what the seatbelt asserts it sees.
const SESSION: &str = "alacritree-e2e";

struct Harness {
    home: tempfile::TempDir,
    server: Child,
    child: Child,
}

impl Harness {
    /// One redirected environment for all three parties. Redirecting it for
    /// only the alacritree child breaks in both directions: its herdr
    /// children would look for the socket under the temp directory where no
    /// server is listening, and a shared-view attach resolves its session
    /// name from whatever herdr lists as running, which would be the
    /// developer's own.
    fn env(command: &mut Command, home: &Path) {
        command
            .env("APPDATA", home)
            .env("LOCALAPPDATA", home)
            .env("XDG_CONFIG_HOME", home)
            .env("XDG_STATE_HOME", home)
            .env("HOME", home)
            .env("HERDR_SESSION", SESSION)
            // wsl.exe forwards nothing by default, so a herdr inside a distro
            // would answer for the user's real session. Forwarding the name
            // points it at one that does not exist there, which reads as a
            // server error and leaves the side retrying rather than
            // answering.
            .env("WSLENV", "HERDR_SESSION")
            // herdr sets these inside every managed pane, and the socket path
            // outranks the session name, so a run started from inside a pane
            // would otherwise drive the pane it is running in.
            .env_remove("HERDR_ENV")
            .env_remove("HERDR_PANE_ID")
            .env_remove("HERDR_TAB_ID")
            .env_remove("HERDR_WORKSPACE_ID")
            .env_remove("HERDR_SOCKET_PATH")
            .env_remove("HERDR_CLIENT_SOCKET_PATH");
    }

    fn start(follow_focus: &str) -> Self {
        let home = tempfile::tempdir().expect("a temp dir");
        write_config(home.path(), follow_focus);

        let mut server = Command::new("herdr");
        Self::env(&mut server, home.path());
        let server = server
            .arg("server")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("herdr is on PATH; these tests are opt-in and a missing binary is a failure");

        let harness_home = home.path().to_path_buf();
        wait_for(|| running_session(&harness_home).is_some())
            .expect("the throwaway herdr server starts");

        let mut child = Command::new(binary());
        Self::env(&mut child, home.path());
        let child = child.spawn().expect("the alacritree binary runs");

        let harness = Self { home, server, child };
        harness.assert_isolated();
        wait_for(|| harness.foreground_is_child()).expect("the window takes foreground");
        harness
    }

    fn home(&self) -> &Path {
        self.home.path()
    }

    /// Before any mutating herdr command: exactly one running session, named
    /// after this test, with its directory under the temp directory. The
    /// isolation failed twice by accident while this recipe was established.
    fn assert_isolated(&self) {
        let session = running_session(self.home()).expect("the throwaway session is running");
        assert_eq!(session["name"], SESSION, "a foreign herdr session is running");
        let dir = PathBuf::from(session["session_dir"].as_str().expect("a session directory"));
        // Windows spells this with a backslash, so comparing a
        // `sessions/<name>` substring would silently never match.
        assert!(
            dir.starts_with(self.home()),
            "the herdr session directory {dir:?} is outside {:?}",
            self.home()
        );
    }

    fn herdr(&self, args: &[&str]) -> Output {
        self.assert_isolated();
        let mut command = Command::new("herdr");
        Self::env(&mut command, self.home());
        command.args(args).output().expect("herdr runs")
    }

    /// Every alacritree command names the child's own socket. Without it a
    /// client that inherited no socket variable finds an instance by listing
    /// the pipe directory, and the developer's live window is in there.
    fn alacritree(&self, args: &[&str]) -> Output {
        let mut command = Command::new(binary());
        Self::env(&mut command, self.home());
        command
            .env("ALACRITREE_SOCKET", format!(r"\\.\pipe\alacritree-{}.sock", self.child.id()))
            .args(args)
            .output()
            .expect("the alacritree binary runs")
    }

    fn sessions(&self) -> Value {
        let out = self.alacritree(&["session", "list", "--json"]);
        serde_json::from_slice(&out.stdout).unwrap_or_else(|err| {
            panic!("session list --json is not JSON: {err}: {}", String::from_utf8_lossy(&out.stdout))
        })
    }

    /// Following only fires while the window is the OS-focused one, and
    /// nothing in the reply carries window focus, so the test asks Win32.
    fn foreground_is_child(&self) -> bool {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetForegroundWindow, GetWindowThreadProcessId,
        };
        let window = unsafe { GetForegroundWindow() };
        if window.is_null() {
            return false;
        }
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(window, &mut pid) };
        pid == self.child.id()
    }
}

impl Drop for Harness {
    /// Order matters. `server stop` first, so every attach client exits on
    /// its own and herdr runs its own shutdown; `Child::kill` is
    /// `TerminateProcess` here and skips every `Drop`. Then the window, whose
    /// conpty children die with the pseudoconsole handle, since `Quit` opens
    /// a dialog rather than exiting. Then the server, if `stop` timed out.
    /// Then the session, which refuses to be deleted while it runs.
    fn drop(&mut self) {
        let mut stop = Command::new("herdr");
        Self::env(&mut stop, self.home.path());
        let _ = stop.args(["server", "stop"]).output();
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = self.server.kill();
        let _ = self.server.wait();
        let mut delete = Command::new("herdr");
        Self::env(&mut delete, self.home.path());
        let _ = delete.args(["session", "delete", SESSION]).output();
    }
}

/// `show_panes` is on because a tab created on the test server runs a plain
/// shell, and an unattached side asks `agent list`, which does not list one.
fn write_config(home: &Path, follow_focus: &str) {
    let dir = home.join("alacritty");
    std::fs::create_dir_all(&dir).expect("the config directory");
    let mut file =
        std::fs::File::create(dir.join("alacritree.toml")).expect("the config file");
    write!(
        file,
        "[integrations.herdr]\nshow_panes = true\nshow_unmatched = true\nattach = \
         \"session\"\nfollow_focus = \"{follow_focus}\"\n"
    )
    .expect("the config is written");
}

fn running_session(home: &Path) -> Option<Value> {
    let mut command = Command::new("herdr");
    Harness::env(&mut command, home);
    let out = command.args(["session", "list", "--json"]).output().ok()?;
    let listing: Value = serde_json::from_slice(&out.stdout).ok()?;
    listing["sessions"]
        .as_array()?
        .iter()
        .find(|s| s["running"].as_bool().unwrap_or(false))
        .cloned()
}

fn wait_for(mut ready: impl FnMut() -> bool) -> Result<(), ()> {
    let deadline = Instant::now() + SETTLE;
    while Instant::now() < deadline {
        if ready() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(())
}

/// The active session for a workspace, as the reply names it.
fn active_terminal(sessions: &Value) -> Option<String> {
    let workspace = sessions["current_workspace"].clone();
    sessions["sessions"].as_array()?.iter().find_map(|s| {
        let here = s["workspace"] == workspace;
        let active = s["is_active_tab"].as_bool().unwrap_or(false);
        (here && active).then(|| s["multiplexer"]["terminal_id"].as_str().map(str::to_string))?
    })
}

/// The default mode follows herdr only from a session already showing its
/// view, so a new pane created while a native session is active must not move
/// the window. This is the guard on Arnaud's unmodified config.
#[test]
#[ignore = "spawns a herdr server and a window; run with the e2e task"]
fn the_default_mode_does_not_follow_from_a_native_session() {
    let harness = Harness::start("herdr");
    let before = active_terminal(&harness.sessions());
    let created = harness.herdr(&["tab", "create", "--focus"]);
    assert!(created.status.success(), "tab create failed: {created:?}");
    // Long enough for two poll intervals plus the quiet gap, so a follow that
    // was going to happen has happened.
    std::thread::sleep(Duration::from_secs(6));
    assert_eq!(active_terminal(&harness.sessions()), before, "the window followed herdr");
}
```

- [ ] **Step 4: Add the devkit task**

In `C:/Users/Lev/Git/github/alacritree/devkit.local.toml`, after the `[tasks.test]` block. It is a task of its own rather than a flag on `test`, so the ordinary suite cannot pick these up by accident. `--test-threads 1` because the window has to hold the foreground alone:

```toml
[tasks.e2e]
description = "Run the opt-in herdr end-to-end suite against a throwaway server"
run = ["cargo", "nextest", "run", "-p", "alacritree", "--locked",
       "--test", "herdr_e2e", "--run-ignored", "ignored-only", "--test-threads", "1"]
```

- [ ] **Step 5: Verify the ordinary suite ignores them**

Run: `devkit run task test`
Expected: PASS, with `herdr_e2e` compiled and its tests reported as skipped.

- [ ] **Step 6: Run the opt-in suite**

Run: `devkit run task e2e`
Expected: PASS. A window appears and closes. If it fails on `the window takes foreground`, another window stole focus; rerun without touching the machine.

- [ ] **Step 7: Format, lint**

```sh
devkit run task fmt
devkit run task clippy
```

- [ ] **Step 8: Commit**

```sh
git add alacritree/tests/herdr_e2e.rs alacritree/Cargo.toml
git commit -m "$(cat <<'EOF'
test(herdr): pilot the real binary against a throwaway server

Following herdr's focus spawns attach clients, switches workspace and
takes terminal focus, none of which an in-crate test can observe. The
harness runs a herdr server and an alacritree window under one
redirected environment so the developer's own sessions stay invisible
to both, and pilots the window through its CLI.

Two seatbelts guard the isolation, which failed twice by accident
while the recipe was established: the herdr session is asserted to be
the only running one and to live under the temp directory, and every
alacritree command names the child's own socket.

The suite is opt-in through its own devkit task, so the ordinary run
cannot pick it up.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

The `devkit.local.toml` change is untracked here and rides the `docs/specs-and-plans` branch; leave it uncommitted in this worktree.

- [ ] **Step 9: Release the claim**

```sh
DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm release \
  /c/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/Cargo.toml
```

---

### Task 4: Restructure `next` around `ViewInputs`

**Files:**
- Modify: `alacritree/src/herdr/view.rs` (`next` at `59-93`, every test below `110`)
- Modify: `alacritree/src/app.rs` (the call site at `1710-1715`)
- Test: `alacritree/src/herdr/view.rs`, the five existing tests

**Interfaces:**
- Consumes: `FollowFocus` from Task 1.
- Produces:

```rust
pub struct ViewInputs<'a> {
    /// The active session and its herdr key, before any direct-attach filter.
    pub active: Option<(SessionId, Option<&'a HerdrKey>, bool)>,
    pub attach: AttachMode,
    pub follow: FollowFocus,
    pub caches: &'a [EndpointCache],
    /// The window is focused, the terminal has pane focus, and neither a
    /// modal nor the palette is open.
    pub attentive: bool,
    /// When direct input last reached the window.
    pub last_direct_input: Option<Instant>,
    pub now: Instant,
    pub busy: bool,
}

pub fn next(&mut self, inputs: ViewInputs<'_>) -> Option<HerdrViewAction>
```

This task changes no behavior. It is the restructure that makes the trail reachable, because `next` returns at `active?` today before it reads a snapshot at all.

- [ ] **Step 1: Claim the files**

```sh
DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm acquire \
  /c/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/herdr/view.rs \
  /c/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/app.rs \
  --note "ViewInputs restructure"
```

- [ ] **Step 2: Add the struct and split the shared-view path out of `next`**

In `alacritree/src/herdr/view.rs`, add the imports and the struct:

```rust
use crate::config::{AttachMode, FollowFocus};
use crate::herdr::EndpointCache;

/// Everything `next` decides from, in one struct because there are more of
/// them than a positional call can carry legibly and clippy allows.  `now` is
/// passed rather than read so the hold-off can be tested without sleeping.
pub struct ViewInputs<'a> {
    /// The active session and its herdr key, before any direct-attach filter.
    /// The filter belongs to the shared-view path: a session that attaches
    /// directly still occupies a pane, and a trail blind to its key would
    /// follow the user to the pane they are already on.
    pub active: Option<(SessionId, Option<&'a HerdrKey>, bool)>,
    pub attach: AttachMode,
    pub follow: FollowFocus,
    pub caches: &'a [EndpointCache],
    /// The window is focused, the terminal has pane focus, and neither a
    /// modal nor the palette is open.
    pub attentive: bool,
    /// When direct input last reached the window.
    pub last_direct_input: Option<Instant>,
    pub now: Instant,
    pub busy: bool,
}
```

Replace `next` with an entry point that delegates, keeping the old body verbatim inside `shared_view`:

```rust
    pub fn next(&mut self, inputs: ViewInputs<'_>) -> Option<HerdrViewAction> {
        match self.shared_view(&inputs)? {
            // The setting governs whether herdr may move alacritree, never
            // whether alacritree may move herdr: a shared view draws the
            // wrong pane without its own focus call.
            HerdrViewAction::Follow(_) if inputs.follow == FollowFocus::Off => None,
            action => Some(action),
        }
    }

    /// The session on screen asking herdr for its own pane, and following
    /// herdr afterwards.  Watermarked against listings that were already in
    /// flight when our own focus call landed.
    fn shared_view(&mut self, inputs: &ViewInputs<'_>) -> Option<HerdrViewAction> {
        let active = inputs
            .active
            .and_then(|(id, key, has_agent)| Some((id, key?, has_agent)))
            .filter(|(_, key, has_agent)| {
                !attaches_directly(&key.side, inputs.attach, *has_agent)
            });
        let visible = active.map(|(id, ..)| id);
        if self.visible != visible {
            self.visible = visible;
            self.focused = None;
            self.follow_after = None;
        }
        if inputs.busy {
            return None;
        }
        let (id, key, has_agent) = active?;
        if needs_view_focus(Some(key), inputs.attach, has_agent, id, self.focused) {
            return Some(HerdrViewAction::Focus(id));
        }
        let cache = inputs.caches.iter().find(|cache| cache.side() == &key.side)?;
        let sampled_at = cache.sampled_at()?;
        if !inputs.attentive || sampled_at <= self.follow_after? {
            return None;
        }
        let focused = cache.agents().iter().find(|agent| agent.focused)?;
        self.follow_after = Some(sampled_at);
        (focused.terminal_id != key.terminal_id).then(|| {
            HerdrViewAction::Follow(HerdrKey {
                side: key.side.clone(),
                terminal_id: focused.terminal_id.clone(),
            })
        })
    }
```

The `attentive` check moved from the caller into here, where it previously lived as a `.filter` on the snapshot in `app.rs`. That is the same gate, expressed once.

- [ ] **Step 3: Update the call site**

In `alacritree/src/app.rs`'s `sync_herdr_view_focus`, replace lines `1693-1715` with:

```rust
        let active = self.active_session_index().map(|index| &self.sessions[index]);
        let key = active.and_then(|session| session.herdr_key.clone());
        let selection = active.map(|session| {
            (session.id, key.as_ref(), self.herdr_pane_has_agent(key.as_ref()))
        });
        let attentive = self.focus == PaneFocus::Terminal
            && !self.is_modal_open()
            && !self.palette.is_open()
            && ctx.input(|input| input.viewport().focused).unwrap_or(true);
        let action = self.herdr_focused_view.next(herdr::ViewInputs {
            active: selection,
            attach: self.config.integrations.herdr.attach,
            follow: self.config.integrations.herdr.follow_focus,
            caches: self.herdr_endpoints.caches(),
            attentive,
            last_direct_input: None,
            now: Instant::now(),
            busy: self.herdr_view_focus.is_some() || !self.pending_herdr_attach.is_empty(),
        });
```

`last_direct_input` is `None` until Task 7 feeds it. Export `ViewInputs` from `alacritree/src/herdr/mod.rs` alongside `HerdrViewAction`; the module re-exports only what its callers name.

The borrow of `self.herdr_endpoints.caches()` and the `&mut self.herdr_focused_view` receiver are two disjoint fields, which the borrow checker accepts only when it can see both. If it objects, bind the caches first with `let caches = self.herdr_endpoints.caches();` and pass `caches` — same disjointness, spelled where the checker can see it. If that still fails, take the field out with `let mut sync = std::mem::take(&mut self.herdr_focused_view);`, call, and put it back; `HerdrViewSync` derives `Default`.

- [ ] **Step 4: Update the five existing tests**

Every `sync.next(active, AttachMode::Session, snapshot, false)` becomes a `ViewInputs`. Add a helper at the top of `mod tests` so the change is mechanical:

```rust
    fn inputs<'a>(
        active: Option<(SessionId, Option<&'a HerdrKey>, bool)>,
        attach: AttachMode,
        caches: &'a [herdr::EndpointCache],
        busy: bool,
    ) -> ViewInputs<'a> {
        ViewInputs {
            active,
            attach,
            follow: FollowFocus::Herdr,
            caches,
            attentive: true,
            last_direct_input: None,
            now: Instant::now(),
            busy,
        }
    }
```

The tests build `&[Agent]` slices today and now need an `EndpointCache` carrying them at a chosen `sampled_at`. Add a test-only constructor to `EndpointCache` in `alacritree/src/herdr/poll.rs`:

```rust
    /// A cache holding one listing at a chosen sample time, for tests that
    /// drive `HerdrViewSync` without a poll behind them.
    #[cfg(test)]
    pub fn for_test(side: Side, agents: Vec<Agent>, sampled_at: Instant) -> Self {
        let mut cache = Self::new(side);
        cache.agents = agents;
        cache.sampled_at = Some(sampled_at);
        cache
    }
```

Match `EndpointCache`'s real field names and its actual constructor; read the struct before writing this. If `new` takes different arguments, use whatever it takes.

Then, for example, `herdr_shared_view_follows_new_tabs_and_refocuses_on_return` becomes:

```rust
        let caches = vec![herdr::EndpointCache::for_test(
            side.clone(),
            panes.clone(),
            focused_at + Duration::from_millis(1),
        )];
        assert_eq!(
            sync.next(inputs(Some((1, Some(&t1), false)), AttachMode::Session, &caches, false)),
            Some(HerdrViewAction::Follow(t2.clone()))
        );
```

Convert all five the same way. Their assertions do not change; only their calls do.

- [ ] **Step 5: Pin the ungated `Focus` arm and both modes**

The `Focus` arm is alacritree telling herdr where to point, so it survives
`"off"`; and the two shipped tests must assert the same thing under every
mode, since the default is what they were written against. Add to `mod tests`:

```rust
    /// The setting governs whether herdr may move alacritree.  A shared view
    /// still owes herdr a focus call, or it draws the wrong pane.
    #[test]
    fn off_still_asks_herdr_for_the_shared_view_pane() {
        let key = HerdrKey { side: herdr::Side::Native, terminal_id: "t1".into() };
        let caches: Vec<herdr::EndpointCache> = Vec::new();
        let mut sync = HerdrViewSync::default();
        let mut off = inputs(Some((1, Some(&key), false)), AttachMode::Session, &caches, false);
        off.follow = FollowFocus::Off;
        assert_eq!(sync.next(off), Some(HerdrViewAction::Focus(1)));
    }

    /// The shipped shared-view behavior is the same in every mode, and no
    /// hold-off stands between it and its follow.
    #[test]
    fn a_shared_view_follows_herdr_in_every_mode() {
        for mode in [FollowFocus::Herdr, FollowFocus::Always] {
            let side = herdr::Side::Native;
            let t1 = HerdrKey { side: side.clone(), terminal_id: "t1".into() };
            let t2 = HerdrKey { side: side.clone(), terminal_id: "t2".into() };
            let panes = herdr::Listing::Panes.parse(
                r#"{"result":{"panes":[
                    {"terminal_id":"t2","pane_id":"w2:p1","tab_id":"w2:t1","focused":true}
                ]}}"#,
            );
            let mut sync = HerdrViewSync::default();
            let focused_at = Instant::now();
            sync.attached(1, focused_at);
            let caches = vec![herdr::EndpointCache::for_test(
                side.clone(),
                panes,
                focused_at + Duration::from_millis(1),
            )];
            let mut at = inputs(Some((1, Some(&t1), false)), AttachMode::Session, &caches, false);
            at.follow = mode;
            assert_eq!(
                sync.next(at),
                Some(HerdrViewAction::Follow(t2.clone())),
                "mode {mode:?} delayed or dropped a shared-view follow"
            );
        }
    }
```

`sync.attached` takes two arguments until Task 5 widens it; add the `None`
key argument to this test as part of that task.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `devkit run task test`
Expected: PASS. Any assertion that changed value means the restructure changed behavior, which it must not.

- [ ] **Step 7: Run the end-to-end guard**

Run: `devkit run task e2e`
Expected: PASS. The default mode still does not follow.

- [ ] **Step 8: Format, lint, commit**

```sh
devkit run task fmt
devkit run task clippy
git add alacritree/src/herdr/view.rs alacritree/src/herdr/poll.rs alacritree/src/herdr/mod.rs alacritree/src/app.rs
git commit -m "$(cat <<'EOF'
refactor(herdr): decide view focus from every endpoint cache

Deriving the snapshot from the active session's key made the decision
unreachable for a session that has no key, and returning before the
snapshot was read put the whole rest of the function behind that. The
caller now hands over every cache and the flags the decision reads,
and the direct-attach filter moves inside the path that wants it.

Behavior is unchanged: a session sharing herdr's view follows it, and
nothing else does.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm release \
  /c/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/herdr/view.rs \
  /c/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/app.rs
```

---

### Task 5: The focus trail

**Files:**
- Modify: `alacritree/src/herdr/view.rs`
- Modify: `alacritree/src/app.rs` (`closed` call at `2032`, `attached` calls at `1678` and `1821`, `settled` calls at `1727` and `1730`)
- Test: `alacritree/src/herdr/view.rs`

**Interfaces:**
- Consumes: `ViewInputs` from Task 4.
- Produces: `HerdrViewSync::moved_focus(&mut self, key: &HerdrKey, at: Instant)`; `closed` becomes `closed(&mut self, id: SessionId, key: Option<&HerdrKey>)`; `attached` becomes `attached(&mut self, id: SessionId, key: Option<&HerdrKey>, at: Instant)`. Task 6 consumes the edge this produces.

At the end of this task the trail records and forms edges, and an edge becomes a `Follow` immediately. Task 6 puts the hold-off in front of it.

- [ ] **Step 1: Claim the files**

```sh
DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm acquire \
  /c/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/herdr/view.rs \
  /c/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/app.rs \
  --note "herdr focus trail"
```

- [ ] **Step 2: Write the failing tests**

In `mod tests` in `alacritree/src/herdr/view.rs`:

```rust
    fn always<'a>(
        active: Option<(SessionId, Option<&'a HerdrKey>, bool)>,
        caches: &'a [herdr::EndpointCache],
        now: Instant,
    ) -> ViewInputs<'a> {
        ViewInputs {
            active,
            attach: AttachMode::Session,
            follow: FollowFocus::Always,
            caches,
            attentive: true,
            last_direct_input: None,
            now,
            busy: false,
        }
    }

    fn one_focused(side: &herdr::Side, terminal_id: &str, at: Instant) -> Vec<herdr::EndpointCache> {
        let panes = herdr::Listing::Panes.parse(&format!(
            r#"{{"result":{{"panes":[
                {{"terminal_id":"{terminal_id}","pane_id":"w1:p1","tab_id":"w1:t1","focused":true}}
            ]}}}}"#
        ));
        vec![herdr::EndpointCache::for_test(side.clone(), panes, at)]
    }

    /// Starting alacritree must never yank the user somewhere, so the first
    /// reading of a side is a baseline rather than a change.
    #[test]
    fn a_first_sight_records_without_following() {
        let side = herdr::Side::Native;
        let now = Instant::now();
        let caches = one_focused(&side, "t1", now);
        let mut sync = HerdrViewSync::default();
        assert_eq!(sync.next(always(Some((1, None, false)), &caches, now)), None);
    }

    #[test]
    fn a_native_session_follows_a_change_on_any_side() {
        let side = herdr::Side::Wsl("ubuntu".into());
        let start = Instant::now();
        let first = one_focused(&side, "t1", start);
        let mut sync = HerdrViewSync::default();
        assert_eq!(sync.next(always(Some((1, None, false)), &first, start)), None);
        let later = start + Duration::from_millis(1);
        let second = one_focused(&side, "t2", later);
        assert_eq!(
            sync.next(always(Some((1, None, false)), &second, later)),
            Some(HerdrViewAction::Follow(HerdrKey {
                side: side.clone(),
                terminal_id: "t2".into(),
            }))
        );
    }

    /// The default mode is what ships, and it must stay blind to a change
    /// made while a native session is active.
    #[test]
    fn the_default_mode_ignores_the_trail() {
        let side = herdr::Side::Native;
        let start = Instant::now();
        let first = one_focused(&side, "t1", start);
        let mut sync = HerdrViewSync::default();
        let mut inputs = always(Some((1, None, false)), &first, start);
        inputs.follow = FollowFocus::Herdr;
        assert_eq!(sync.next(inputs), None);
        let later = start + Duration::from_millis(1);
        let second = one_focused(&side, "t2", later);
        let mut inputs = always(Some((1, None, false)), &second, later);
        inputs.follow = FollowFocus::Herdr;
        assert_eq!(sync.next(inputs), None);
    }

    /// A change the user is already looking at is not somewhere to go.
    #[test]
    fn a_change_onto_the_active_pane_records_without_following() {
        let side = herdr::Side::Native;
        let key = HerdrKey { side: side.clone(), terminal_id: "t2".into() };
        let start = Instant::now();
        let first = one_focused(&side, "t1", start);
        let mut sync = HerdrViewSync::default();
        assert_eq!(sync.next(always(Some((1, Some(&key), false)), &first, start)), None);
        let later = start + Duration::from_millis(1);
        let second = one_focused(&side, "t2", later);
        assert_eq!(sync.next(always(Some((1, Some(&key), false)), &second, later)), None);
    }

    /// A listing already in flight when alacritree moved herdr's focus
    /// reports the old pane, and acting on it would run the change backwards.
    #[test]
    fn a_sample_older_than_the_stamp_forms_no_edge() {
        let side = herdr::Side::Native;
        let start = Instant::now();
        let mut sync = HerdrViewSync::default();
        let first = one_focused(&side, "t1", start);
        assert_eq!(sync.next(always(Some((1, None, false)), &first, start)), None);
        let moved = start + Duration::from_millis(5);
        sync.moved_focus(&HerdrKey { side: side.clone(), terminal_id: "t3".into() }, moved);
        let in_flight = one_focused(&side, "t2", start + Duration::from_millis(2));
        assert_eq!(
            sync.next(always(Some((1, None, false)), &in_flight, moved + Duration::from_millis(1))),
            None
        );
    }

    /// A side that stops answering empties its listing, and a side whose
    /// distro stopped loses its cache entirely; neither is a focus change.
    #[test]
    fn a_silent_or_vanished_side_forms_no_edge() {
        let side = herdr::Side::Wsl("ubuntu".into());
        let start = Instant::now();
        let first = one_focused(&side, "t1", start);
        let mut sync = HerdrViewSync::default();
        assert_eq!(sync.next(always(Some((1, None, false)), &first, start)), None);
        let later = start + Duration::from_millis(1);
        let silent = vec![herdr::EndpointCache::for_test(side.clone(), Vec::new(), later)];
        assert_eq!(sync.next(always(Some((1, None, false)), &silent, later)), None);
        let gone: Vec<herdr::EndpointCache> = Vec::new();
        assert_eq!(sync.next(always(Some((1, None, false)), &gone, later)), None);
        // The side comes back: its first reading is a baseline again, not the
        // change it looks like against the entry that used to be there.
        let back = one_focused(&side, "t9", later + Duration::from_millis(1));
        assert_eq!(
            sync.next(always(Some((1, None, false)), &back, later + Duration::from_millis(1))),
            None
        );
    }

    /// Following acts on any reachable side, and a side nobody touched is
    /// not a change.
    #[test]
    fn a_change_on_one_side_leaves_a_quiet_side_alone() {
        let native = herdr::Side::Native;
        let wsl = herdr::Side::Wsl("ubuntu".into());
        let start = Instant::now();
        let mut sync = HerdrViewSync::default();
        let mut first = one_focused(&native, "n1", start);
        first.extend(one_focused(&wsl, "w1", start));
        assert_eq!(sync.next(always(Some((1, None, false)), &first, start)), None);

        let later = start + Duration::from_millis(1);
        let mut second = one_focused(&native, "n1", later);
        second.extend(one_focused(&wsl, "w2", later));
        assert_eq!(
            sync.next(always(Some((1, None, false)), &second, later)),
            Some(HerdrViewAction::Follow(HerdrKey {
                side: wsl.clone(),
                terminal_id: "w2".into(),
            })),
            "the side that moved is the one to go to"
        );
    }

    /// The shared-view path owns a session that shows herdr's view; the trail
    /// speaks only when that path has nothing to say.
    #[test]
    fn the_shared_view_path_wins_over_the_trail() {
        let side = herdr::Side::Native;
        let key = HerdrKey { side: side.clone(), terminal_id: "t1".into() };
        let start = Instant::now();
        let caches = one_focused(&side, "t2", start);
        let mut sync = HerdrViewSync::default();
        assert_eq!(
            sync.next(always(Some((1, Some(&key), false)), &caches, start)),
            Some(HerdrViewAction::Focus(1))
        );
    }
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `devkit run task test`
Expected: FAIL, `no method named moved_focus`.

- [ ] **Step 4: Add the trail**

In `alacritree/src/herdr/view.rs`:

```rust
use std::collections::HashMap;

/// Where a side's focus was last established, and when.  The stamp is a
/// watermark: a listing sampled at or before it cannot form an edge, so one
/// already in flight when alacritree moved herdr's focus cannot run the
/// change backwards.
struct TrailEntry {
    terminal_id: String,
    stamped_at: Instant,
}
```

Add the field to `HerdrViewSync`:

```rust
    trail: HashMap<Side, TrailEntry>,
```

And the methods:

```rust
    /// Record where herdr's focus now is, without proposing anything.  Every
    /// path that moves herdr's focus calls this, so the move alacritree asked
    /// for is never mistaken for one the user made inside herdr.
    pub fn moved_focus(&mut self, key: &HerdrKey, at: Instant) {
        self.trail.insert(
            key.side.clone(),
            TrailEntry { terminal_id: key.terminal_id.clone(), stamped_at: at },
        );
    }

    /// The first side whose focused pane differs from what the trail holds.
    /// Two sides changing between one frame and the next is rare enough that
    /// a deliberate tiebreak would be inventing a rule nobody can observe,
    /// and the other side's change is still a change on the next frame.
    fn trail_edge(&mut self, inputs: &ViewInputs<'_>) -> Option<HerdrKey> {
        let live: Vec<&Side> = inputs.caches.iter().map(EndpointCache::side).collect();
        self.trail.retain(|side, _| live.contains(&side));
        let active_key = inputs.active.and_then(|(_, key, _)| key);
        let mut edge = None;
        for cache in inputs.caches {
            let Some(sampled_at) = cache.sampled_at() else { continue };
            // A side reporting no focused pane leaves its entry alone: under
            // `show_panes = false` an unattached side lists only panes herdr
            // found an agent in, so herdr's focus sitting on an unlisted pane
            // reads the same as a failed poll.
            let Some(focused) = cache.agents().iter().find(|agent| agent.focused) else {
                continue;
            };
            let key =
                HerdrKey { side: cache.side().clone(), terminal_id: focused.terminal_id.clone() };
            match self.trail.get(cache.side()) {
                // An id appearing where there was no entry is first sight.
                None => self.moved_focus(&key, sampled_at),
                Some(entry) => {
                    if entry.terminal_id == focused.terminal_id || sampled_at <= entry.stamped_at {
                        continue;
                    }
                    if active_key == Some(&key) {
                        self.moved_focus(&key, sampled_at);
                        continue;
                    }
                    edge.get_or_insert(key);
                },
            }
        }
        edge
    }
```

Extend `next`:

```rust
    pub fn next(&mut self, inputs: ViewInputs<'_>) -> Option<HerdrViewAction> {
        if let Some(action) = self.shared_view(&inputs) {
            return match action {
                HerdrViewAction::Follow(_) if inputs.follow == FollowFocus::Off => None,
                action => Some(action),
            };
        }
        if inputs.busy || !inputs.attentive {
            return None;
        }
        let edge = self.trail_edge(&inputs)?;
        (inputs.follow == FollowFocus::Always).then(|| HerdrViewAction::Follow(edge))
    }
```

Recording runs on every attentive frame whatever the mode, so switching to `"always"` at runtime starts from a warm trail rather than from a change the user never saw. Only proposing is gated.

- [ ] **Step 5: Widen `closed` and `attached`, and stamp at the three call sites**

In `view.rs`:

```rust
    pub fn closed(&mut self, id: SessionId, key: Option<&HerdrKey>) {
        if self.visible == Some(id) {
            self.visible = None;
            self.follow_after = None;
        }
        if self.focused == Some(id) {
            self.focused = None;
        }
        let _ = key;
    }

    pub fn attached(&mut self, id: SessionId, key: Option<&HerdrKey>, at: Instant) {
        if let Some(key) = key {
            self.moved_focus(key, at);
        }
        self.visible = Some(id);
        self.settled(id, true, at);
    }
```

`closed` takes the key now because Task 6 needs it; the `let _ = key;` goes away there.

In `alacritree/src/app.rs`:

- Line `1678`, inside `open_herdr_session`: the gesture focused the pane before attaching whether or not the result is a shared view, so the stamp is unconditional while `attached` stays inside the `shared_view` branch.

```rust
                self.herdr_focused_view.moved_focus(&key, Instant::now());
                if shared_view {
                    self.herdr_focused_view.attached(id, Some(&key), Instant::now());
                }
```

`key` is moved into `session.bind_herdr(key)` just above, so clone it before that call and stamp with the clone.

- Line `1821`, at the end of `follow_herdr_view`: `self.herdr_focused_view.attached(id, Some(&key), Instant::now());`. `key` is the parameter and is still in scope.
- Line `2032`, in `close_session`: `self.herdr_focused_view.closed(id, self.sessions[idx].herdr_key.as_ref());`. The borrow of `self.sessions` and the `&mut` receiver collide, so read the key into a local above the call.
- Lines `1727` and `1730`, the `settled` calls: after a successful focus, stamp too, because the `Focus` arm is alacritree moving herdr.

```rust
                    let succeeded = result.is_ok();
                    if let Err(e) = result {
                        log::warn!("{e}");
                    }
                    if succeeded {
                        if let Some(key) = &key {
                            self.herdr_focused_view.moved_focus(key, Instant::now());
                        }
                    }
                    self.herdr_focused_view.settled(pending.session, succeeded, Instant::now());
```

- Lines `12197` and `12417`, the two tests calling `attached`: pass `None` for the key.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `devkit run task test`
Expected: PASS.

- [ ] **Step 7: Format, lint, commit**

```sh
devkit run task fmt
devkit run task clippy
git add alacritree/src/herdr/view.rs alacritree/src/app.rs
git commit -m "$(cat <<'EOF'
feat(herdr): follow herdr's focus from a native session

A focus change made inside herdr reached only a session already
showing herdr's view, while the endpoint poll held the answer for
every other one. A per-side trail of the last focused pane turns a
difference between two samples into somewhere to go.

Every path that moves herdr's focus stamps the trail, so alacritree
never follows itself, and the stamp is a watermark on samples, so a
listing already in flight cannot run the change backwards. Recording
happens whatever the mode; only proposing is gated on "always".

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm release \
  /c/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/herdr/view.rs \
  /c/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/app.rs
```

---

### Task 6: The pending follow

**Files:**
- Modify: `alacritree/src/herdr/view.rs`
- Test: `alacritree/src/herdr/view.rs`

**Interfaces:**
- Consumes: the edge from Task 5.
- Produces: no new public method. `next` now emits a trail `Follow` on a later frame than the edge, or not at all.

- [ ] **Step 1: Claim the file**

```sh
DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm acquire \
  /c/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/herdr/view.rs \
  --note "herdr pending follow hold-off"
```

- [ ] **Step 2: Write the failing tests**

`always` from Task 5 gains a `last_direct_input`. Add these to `mod tests`:

```rust
    /// A pane a script created can arrive mid-command, and following moves
    /// the keyboard, so it waits for the typing to stop.
    #[test]
    fn a_follow_waits_for_a_gap_in_typing() {
        let side = herdr::Side::Native;
        let start = Instant::now();
        let mut sync = HerdrViewSync::default();
        let first = one_focused(&side, "t1", start);
        assert_eq!(sync.next(always(Some((1, None, false)), &first, start)), None);

        let sampled = start + Duration::from_millis(1);
        let second = one_focused(&side, "t2", sampled);
        let mut typing = always(Some((1, None, false)), &second, sampled);
        typing.last_direct_input = Some(sampled);
        assert_eq!(sync.next(typing), None, "proposed, not delivered");

        // Still typing half a second later.
        let mut typing = always(Some((1, None, false)), &second, sampled + Duration::from_millis(500));
        typing.last_direct_input = Some(sampled + Duration::from_millis(500));
        assert_eq!(sync.next(typing), None);

        // The gap arrives.
        let mut quiet =
            always(Some((1, None, false)), &second, sampled + Duration::from_millis(1300));
        quiet.last_direct_input = Some(sampled + Duration::from_millis(500));
        assert_eq!(
            sync.next(quiet),
            Some(HerdrViewAction::Follow(HerdrKey {
                side: side.clone(),
                terminal_id: "t2".into(),
            }))
        );
    }

    /// Moving the user long after the change is worse than not moving them.
    #[test]
    fn a_follow_expires_if_the_gap_never_comes() {
        let side = herdr::Side::Native;
        let start = Instant::now();
        let mut sync = HerdrViewSync::default();
        let first = one_focused(&side, "t1", start);
        assert_eq!(sync.next(always(Some((1, None, false)), &first, start)), None);
        let sampled = start + Duration::from_millis(1);
        let second = one_focused(&side, "t2", sampled);
        let mut at = sampled;
        // Typing without pause, in 200 ms frames, past the expiry.
        for _ in 0..60 {
            let mut typing = always(Some((1, None, false)), &second, at);
            typing.last_direct_input = Some(at);
            assert_eq!(sync.next(typing), None);
            at += Duration::from_millis(200);
        }
        // The typing stops, and the change is stale rather than pending.
        let mut quiet = always(Some((1, None, false)), &second, at + Duration::from_secs(2));
        quiet.last_direct_input = Some(at);
        assert_eq!(sync.next(quiet), None);
    }

    /// The pane herdr is on now is the only one worth going to.
    #[test]
    fn a_second_change_retargets_the_pending_follow_and_restarts_its_clock() {
        let side = herdr::Side::Native;
        let start = Instant::now();
        let mut sync = HerdrViewSync::default();
        let first = one_focused(&side, "t1", start);
        assert_eq!(sync.next(always(Some((1, None, false)), &first, start)), None);

        let a = start + Duration::from_millis(1);
        let second = one_focused(&side, "t2", a);
        let mut typing = always(Some((1, None, false)), &second, a);
        typing.last_direct_input = Some(a);
        assert_eq!(sync.next(typing), None);

        let b = a + Duration::from_millis(600);
        let third = one_focused(&side, "t3", b);
        let mut typing = always(Some((1, None, false)), &third, b);
        typing.last_direct_input = Some(b);
        assert_eq!(sync.next(typing), None);

        let mut quiet = always(Some((1, None, false)), &third, b + Duration::from_millis(800));
        quiet.last_direct_input = Some(b);
        assert_eq!(
            sync.next(quiet),
            Some(HerdrViewAction::Follow(HerdrKey {
                side: side.clone(),
                terminal_id: "t3".into(),
            })),
            "the newest change wins"
        );
    }

    /// The proposal was made against a situation that no longer holds.
    #[test]
    fn a_pending_follow_is_dropped_when_the_active_session_changes() {
        let side = herdr::Side::Native;
        let start = Instant::now();
        let mut sync = HerdrViewSync::default();
        let first = one_focused(&side, "t1", start);
        assert_eq!(sync.next(always(Some((1, None, false)), &first, start)), None);
        let sampled = start + Duration::from_millis(1);
        let second = one_focused(&side, "t2", sampled);
        let mut typing = always(Some((1, None, false)), &second, sampled);
        typing.last_direct_input = Some(sampled);
        assert_eq!(sync.next(typing), None);
        let later = sampled + Duration::from_millis(1300);
        let mut quiet = always(Some((7, None, false)), &second, later);
        quiet.last_direct_input = Some(sampled);
        assert_eq!(sync.next(quiet), None);
    }

    /// Following a row the user just closed would respawn its attach client.
    #[test]
    fn a_pending_follow_is_dropped_when_its_target_closes() {
        let side = herdr::Side::Native;
        let target = HerdrKey { side: side.clone(), terminal_id: "t2".into() };
        let start = Instant::now();
        let mut sync = HerdrViewSync::default();
        let first = one_focused(&side, "t1", start);
        assert_eq!(sync.next(always(Some((1, None, false)), &first, start)), None);
        let sampled = start + Duration::from_millis(1);
        let second = one_focused(&side, "t2", sampled);
        let mut typing = always(Some((1, None, false)), &second, sampled);
        typing.last_direct_input = Some(sampled);
        assert_eq!(sync.next(typing), None);
        sync.closed(9, Some(&target));
        let mut quiet =
            always(Some((1, None, false)), &second, sampled + Duration::from_millis(1300));
        quiet.last_direct_input = Some(sampled);
        assert_eq!(sync.next(quiet), None);
    }

    /// An attach on Windows can hold busy for seconds, and time the user
    /// never saw must not spend the follow's budget.
    #[test]
    fn a_busy_frame_neither_advances_nor_expires_the_pending_follow() {
        let side = herdr::Side::Native;
        let start = Instant::now();
        let mut sync = HerdrViewSync::default();
        let first = one_focused(&side, "t1", start);
        assert_eq!(sync.next(always(Some((1, None, false)), &first, start)), None);
        let sampled = start + Duration::from_millis(1);
        let second = one_focused(&side, "t2", sampled);
        let mut typing = always(Some((1, None, false)), &second, sampled);
        typing.last_direct_input = Some(sampled);
        assert_eq!(sync.next(typing), None);

        let mut busy = always(Some((1, None, false)), &second, sampled + Duration::from_secs(30));
        busy.last_direct_input = Some(sampled);
        busy.busy = true;
        assert_eq!(sync.next(busy), None);

        // The quiet gap is measured from the frames the user was present for,
        // so the follow survives the attach and lands after it.
        let mut quiet = always(
            Some((1, None, false)),
            &second,
            sampled + Duration::from_secs(30) + Duration::from_millis(800),
        );
        quiet.last_direct_input = Some(sampled);
        assert!(matches!(sync.next(quiet), Some(HerdrViewAction::Follow(_))));
    }

    /// Time spent in another window is the catch-up-on-return case the trail
    /// exists to preserve, not time the follow should age through.
    #[test]
    fn an_inattentive_frame_neither_advances_nor_expires_the_pending_follow() {
        let side = herdr::Side::Native;
        let start = Instant::now();
        let mut sync = HerdrViewSync::default();
        let first = one_focused(&side, "t1", start);
        assert_eq!(sync.next(always(Some((1, None, false)), &first, start)), None);
        let sampled = start + Duration::from_millis(1);
        let second = one_focused(&side, "t2", sampled);
        let mut typing = always(Some((1, None, false)), &second, sampled);
        typing.last_direct_input = Some(sampled);
        assert_eq!(sync.next(typing), None);

        let mut away = always(Some((1, None, false)), &second, sampled + Duration::from_secs(60));
        away.last_direct_input = Some(sampled);
        away.attentive = false;
        assert_eq!(sync.next(away), None);

        let mut back = always(
            Some((1, None, false)),
            &second,
            sampled + Duration::from_secs(60) + Duration::from_millis(800),
        );
        back.last_direct_input = Some(sampled);
        assert!(matches!(sync.next(back), Some(HerdrViewAction::Follow(_))));
    }

    /// A follow the app could not deliver leaves the trail unstamped, so the
    /// same change is proposed again rather than lost.
    #[test]
    fn an_undelivered_follow_is_proposed_again() {
        let side = herdr::Side::Native;
        let start = Instant::now();
        let mut sync = HerdrViewSync::default();
        let first = one_focused(&side, "t1", start);
        assert_eq!(sync.next(always(Some((1, None, false)), &first, start)), None);
        let sampled = start + Duration::from_millis(1);
        let second = one_focused(&side, "t2", sampled);
        assert_eq!(sync.next(always(Some((1, None, false)), &second, sampled)), None);
        let quiet = sampled + Duration::from_millis(800);
        assert!(matches!(
            sync.next(always(Some((1, None, false)), &second, quiet)),
            Some(HerdrViewAction::Follow(_))
        ));
        // The app never called `attached`, so nothing stamped the trail.
        let again = quiet + Duration::from_millis(800);
        assert!(matches!(
            sync.next(always(Some((1, None, false)), &second, again)),
            Some(HerdrViewAction::Follow(_))
        ));
    }

    /// Giving up is a decision, and without recording it the same stale
    /// change would be re-proposed forever.
    #[test]
    fn an_expired_follow_stamps_the_trail() {
        let side = herdr::Side::Native;
        let start = Instant::now();
        let mut sync = HerdrViewSync::default();
        let first = one_focused(&side, "t1", start);
        assert_eq!(sync.next(always(Some((1, None, false)), &first, start)), None);
        let sampled = start + Duration::from_millis(1);
        let second = one_focused(&side, "t2", sampled);
        let mut at = sampled;
        for _ in 0..60 {
            let mut typing = always(Some((1, None, false)), &second, at);
            typing.last_direct_input = Some(at);
            assert_eq!(sync.next(typing), None);
            at += Duration::from_millis(200);
        }
        for _ in 0..10 {
            at += Duration::from_secs(1);
            assert_eq!(sync.next(always(Some((1, None, false)), &second, at)), None);
        }
    }
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `devkit run task test`
Expected: FAIL. `a_follow_waits_for_a_gap_in_typing` returns a `Follow` on the first frame, because Task 5 emits the edge immediately.

- [ ] **Step 4: Add the pending follow**

In `alacritree/src/herdr/view.rs`:

```rust
/// How long the user has to stop typing before a follow lands.  Long enough
/// to clear the pause between keystrokes and short enough that a deliberate
/// pause is not mistaken for continued work.
const FOLLOW_QUIET_GAP: Duration = Duration::from_millis(750);

/// How long a proposed follow keeps waiting.  Matches `PROBE_GRACE`, the
/// codebase's one existing answer to "the user is active".  Past it the
/// change is stale, and moving the user then is worse than not moving them.
const FOLLOW_EXPIRY: Duration = Duration::from_secs(10);

/// A follow that has been proposed and is waiting for the user to stop
/// typing.  Both clocks count attentive time only: counting wall-clock time
/// would expire a follow while the user was in another window, which is the
/// catch-up-on-return case the trail exists to preserve.
struct PendingFollow {
    key: HerdrKey,
    /// The session that was active when this was proposed.  A different one
    /// means the proposal no longer describes the situation.
    active: Option<SessionId>,
    /// Attentive time since the last direct input.
    quiet: Duration,
    /// Attentive time since the proposal.
    age: Duration,
}
```

Add to `HerdrViewSync`:

```rust
    pending: Option<PendingFollow>,
    /// When `next` last counted attentive time, so a frame's contribution is
    /// the gap since the previous one rather than a fixed tick.
    ticked_at: Option<Instant>,
    /// The direct-input reading the current `quiet` was measured from.
    input_seen: Option<Instant>,
```

Rewrite `next`:

```rust
    pub fn next(&mut self, inputs: ViewInputs<'_>) -> Option<HerdrViewAction> {
        let elapsed = self.tick(&inputs);
        if let Some(action) = self.shared_view(&inputs) {
            return match action {
                HerdrViewAction::Follow(_) if inputs.follow == FollowFocus::Off => None,
                action => Some(action),
            };
        }
        if inputs.busy || !inputs.attentive {
            return None;
        }
        if let Some(edge) = self.trail_edge(&inputs) {
            self.propose(edge, &inputs);
        }
        self.deliver(&inputs, elapsed)
    }

    /// Attentive time since the previous frame.  Counted on every call so a
    /// busy or inattentive stretch is skipped rather than back-charged to the
    /// pending follow on the frame after it.
    fn tick(&mut self, inputs: &ViewInputs<'_>) -> Duration {
        let previous = self.ticked_at.replace(inputs.now);
        if inputs.busy || !inputs.attentive {
            return Duration::ZERO;
        }
        previous.map_or(Duration::ZERO, |previous| inputs.now.saturating_duration_since(previous))
    }

    /// The newest change wins: a second edge replaces the target and restarts
    /// the clock, because the pane herdr is on now is the only one worth
    /// going to.  Re-seeing the same edge, which happens every frame until
    /// the trail is stamped, changes nothing.
    fn propose(&mut self, key: HerdrKey, inputs: &ViewInputs<'_>) {
        if self.pending.as_ref().is_some_and(|pending| pending.key == key) {
            return;
        }
        self.pending = Some(PendingFollow {
            key,
            active: inputs.active.map(|(id, ..)| id),
            quiet: Duration::ZERO,
            age: Duration::ZERO,
        });
        self.input_seen = inputs.last_direct_input;
    }

    fn deliver(&mut self, inputs: &ViewInputs<'_>, elapsed: Duration) -> Option<HerdrViewAction> {
        if inputs.follow != FollowFocus::Always {
            self.pending = None;
            return None;
        }
        let active = inputs.active.map(|(id, ..)| id);
        // The proposal was made against a situation that no longer holds.
        if self.pending.as_ref().is_some_and(|pending| pending.active != active) {
            self.pending = None;
            return None;
        }
        let input_moved = self.input_seen != inputs.last_direct_input;
        if input_moved {
            self.input_seen = inputs.last_direct_input;
        }
        let (expired, ready) = {
            let pending = self.pending.as_mut()?;
            pending.quiet = if input_moved { Duration::ZERO } else { pending.quiet + elapsed };
            pending.age += elapsed;
            (pending.age >= FOLLOW_EXPIRY, pending.quiet >= FOLLOW_QUIET_GAP)
        };
        if expired {
            let key = self.pending.take()?.key;
            // Recording the decision not to go, or the same stale change
            // would be re-proposed on every frame that follows.
            self.moved_focus(&key, inputs.now);
            return None;
        }
        if !ready {
            return None;
        }
        let key = self.pending.take()?.key;
        // The target may have become the active pane while this waited.
        inputs
            .active
            .and_then(|(_, active, _)| active)
            .is_none_or(|active| active != &key)
            .then_some(HerdrViewAction::Follow(key))
    }
```

Drop the pending in `closed` when the session that closed held its target:

```rust
    pub fn closed(&mut self, id: SessionId, key: Option<&HerdrKey>) {
        if self.visible == Some(id) {
            self.visible = None;
            self.follow_after = None;
        }
        if self.focused == Some(id) {
            self.focused = None;
        }
        // A follow to a row the user just closed would respawn its attach
        // client.
        if key.is_some_and(|key| self.pending.as_ref().is_some_and(|p| &p.key == key)) {
            self.pending = None;
        }
    }
```

Note that `deliver` does not stamp the trail on a successful emit. Stamping is `attached`'s job, which the app calls once the follow actually landed; a follow the app could not deliver leaves the trail alone and is proposed again on the next frame past the gap.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `devkit run task test`
Expected: PASS.

- [ ] **Step 6: Format, lint, commit**

```sh
devkit run task fmt
devkit run task clippy
git add alacritree/src/herdr/view.rs
git commit -m "$(cat <<'EOF'
feat(herdr): hold a follow until the typing stops

Following takes the keyboard, and in "always" mode a pane a script
created can arrive mid-command, so acting on the change the moment it
is seen lands the tail of a half-typed line in a shell the user never
chose. A proposed follow now waits for a gap in direct input and is
dropped rather than delivered late once the change is stale.

Both clocks count only frames the user was present for, so an attach
that holds the window busy and a stretch spent in another window
neither spend the budget nor age the proposal.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm release \
  /c/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/herdr/view.rs
```

---

### Task 7: The direct-input clock, and the end-to-end proof

**Files:**
- Modify: `alacritree/src/app.rs` (the field block near `742`, the constructor near `1049`, the event drain near `11535`, the `next` call site)
- Modify: `alacritree/tests/herdr_e2e.rs`
- Test: `alacritree/tests/herdr_e2e.rs`

**Interfaces:**
- Consumes: everything from Tasks 1 through 6.
- Produces: `AlacritreeApp::last_direct_input: Option<Instant>`, fed into `ViewInputs`.

- [ ] **Step 1: Claim the files**

```sh
DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm acquire \
  /c/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/app.rs \
  /c/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/tests/herdr_e2e.rs \
  --note "direct-input clock and the follow e2e test"
```

- [ ] **Step 2: Write the failing end-to-end test**

Append to `alacritree/tests/herdr_e2e.rs`:

```rust
/// The whole feature, through the real window: a pane created inside herdr
/// while a native session is active pulls alacritree onto it.
#[test]
#[ignore = "spawns a herdr server and a window; run with the e2e task"]
fn always_follows_a_new_pane_from_a_native_session() {
    let harness = Harness::start("always");
    let before = active_terminal(&harness.sessions());
    // Without --focus the server does not move, and no edge forms, so the
    // test would fail for a reason that is not the feature.
    let created = harness.herdr(&["tab", "create", "--focus"]);
    assert!(created.status.success(), "tab create failed: {created:?}");
    let landed = wait_for(|| {
        let sessions = harness.sessions();
        active_terminal(&sessions).is_some() && active_terminal(&sessions) != before
    });
    assert!(landed.is_ok(), "the window never followed herdr: {:#}", harness.sessions());
}
```

- [ ] **Step 3: Run it to verify it fails**

Run: `devkit run task e2e`
Expected: FAIL on `always_follows_a_new_pane_from_a_native_session` with "the window never followed herdr". `the_default_mode_does_not_follow_from_a_native_session` still passes.

If it fails instead on `the window takes foreground`, that is the harness and not the feature; rerun without touching the machine.

- [ ] **Step 4: Add the direct-input clock**

In `alacritree/src/app.rs`, beside `last_input` near line `742`:

```rust
    /// When input the user aimed at this window last arrived.  Distinct from
    /// `last_input`, which advances on any event at all, including pointer
    /// motion and window focus, and which the liveness probe reads as its
    /// grace period.
    last_direct_input: Option<Instant>,
```

In the constructor near line `1049`, beside `last_input: Instant::now(),`:

```rust
            last_direct_input: None,
```

Replace the event drain near line `11535`. Bare pointer motion is left out deliberately: hovering the mouse over the window would otherwise hold a follow off forever. `WindowFocused` is in, so a follow waiting while the user was elsewhere lands shortly after they return rather than the instant they do:

```rust
        let (any_event, direct_input) = ctx.input(|i| {
            let direct = i.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Key { .. }
                        | egui::Event::Text(_)
                        | egui::Event::Paste(_)
                        | egui::Event::Copy
                        | egui::Event::Cut
                        | egui::Event::Scroll(_)
                        | egui::Event::MouseWheel { .. }
                        | egui::Event::Zoom(_)
                        | egui::Event::PointerButton { .. }
                        | egui::Event::WindowFocused(_)
                )
            });
            (!i.events.is_empty(), direct)
        });
        if any_event {
            self.last_input = Instant::now();
        }
        if direct_input {
            self.last_direct_input = Some(Instant::now());
        }
```

Check the `egui::Event` variants against the version this workspace pins before writing this list; a variant that does not exist is a compile error, and one that was renamed is a silent gap. `devkit run task check` catches the first, not the second.

- [ ] **Step 5: Feed it into `ViewInputs`**

In `sync_herdr_view_focus`, `last_direct_input: None` becomes `last_direct_input: self.last_direct_input`.

- [ ] **Step 6: Run the unit tests**

Run: `devkit run task test`
Expected: PASS.

- [ ] **Step 7: Run the end-to-end suite to verify it passes**

Run: `devkit run task e2e`
Expected: PASS, both tests.

- [ ] **Step 8: Format, lint, commit**

```sh
devkit run task fmt
devkit run task clippy
git add alacritree/src/app.rs alacritree/tests/herdr_e2e.rs
git commit -m "$(cat <<'EOF'
feat(herdr): hold a follow off while the user is at the keyboard

The existing input clock advances on any event, pointer motion and
window focus included, and the liveness probe already reads it as its
grace period, so a debounce hung on it would both fire while the mouse
merely rested over the window and change what the probe means.

A second clock moves only for input the user aimed at the window.
Bare pointer motion is left out, or hovering would hold a follow off
forever; window focus is in, so one waiting while the user was
elsewhere lands shortly after they return.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm release \
  /c/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/app.rs \
  /c/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/tests/herdr_e2e.rs
```

---

### Task 8: The hold-off, end to end

**Files:**
- Modify: `alacritree/tests/herdr_e2e.rs`
- Test: `alacritree/tests/herdr_e2e.rs`

**Interfaces:**
- Consumes: the harness from Task 3 and the clock from Task 7.
- Produces: nothing later tasks use. This is the last task.

`send-text` writes to the PTY and `run-action` dispatches a binding action; neither produces an egui event, so neither moves the hold-off clock. This is the one test that would catch a debounce wired to the wrong clock, and it needs real input.

- [ ] **Step 1: Claim the file**

```sh
DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm acquire \
  /c/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/tests/herdr_e2e.rs \
  --note "SendInput hold-off e2e test"
```

- [ ] **Step 2: Write the failing test**

Append to `alacritree/tests/herdr_e2e.rs`:

```rust
/// One keystroke into the foreground window, through the OS.
fn type_a_key(harness: &Harness) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VK_SHIFT,
    };
    // Typing goes to whatever holds the foreground, so this runs only once
    // the window is known to be the child's. Shift is chosen because it
    // reaches egui as an event without putting a character in the shell.
    assert!(harness.foreground_is_child(), "refusing to type into a foreign window");
    let mut down: INPUT = unsafe { std::mem::zeroed() };
    down.r#type = INPUT_KEYBOARD;
    down.Anonymous.ki = KEYBDINPUT {
        wVk: VK_SHIFT,
        wScan: 0,
        dwFlags: 0,
        time: 0,
        dwExtraInfo: 0,
    };
    let mut up = down;
    up.Anonymous.ki.dwFlags = KEYEVENTF_KEYUP;
    let inputs = [down, up];
    unsafe {
        SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32)
    };
}

/// A follow arriving while the user is typing waits, and lands once they
/// stop. This is the one test that catches a debounce wired to the clock
/// that also moves for pointer motion.
#[test]
#[ignore = "types into the foreground window; run with the e2e task"]
fn a_follow_waits_while_the_user_types() {
    let harness = Harness::start("always");
    let before = active_terminal(&harness.sessions());
    let created = harness.herdr(&["tab", "create", "--focus"]);
    assert!(created.status.success(), "tab create failed: {created:?}");

    // Type continuously for longer than the quiet gap, well short of the
    // expiry, and assert the window has not moved.
    let typing_until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < typing_until {
        type_a_key(&harness);
        std::thread::sleep(Duration::from_millis(150));
    }
    assert_eq!(
        active_terminal(&harness.sessions()),
        before,
        "the window followed herdr while the user was typing"
    );

    let landed = wait_for(|| {
        let sessions = harness.sessions();
        active_terminal(&sessions).is_some() && active_terminal(&sessions) != before
    });
    assert!(landed.is_ok(), "the follow never landed after the typing stopped");
}
```

Read the `windows-sys` 0.59 signature for `SendInput` and the field names of `INPUT` and `KEYBDINPUT` before writing this; the union member and the `type` field are spelled differently across versions, and a wrong name is a compile error rather than a silent pass.

- [ ] **Step 3: Run it**

Run: `devkit run task e2e`
Expected: PASS, all three tests. If `a_follow_waits_while_the_user_types` fails on the first assertion, the hold-off is reading the wrong clock; if it fails on the second, the expiry is firing inside five seconds.

- [ ] **Step 4: Format, lint, and run everything**

```sh
devkit run task fmt
devkit run task clippy
devkit run task test
devkit run task e2e
```

- [ ] **Step 5: Commit**

```sh
git add alacritree/tests/herdr_e2e.rs
git commit -m "$(cat <<'EOF'
test(herdr): prove the hold-off against real keyboard input

No CLI surface produces an egui event: send-text writes to the PTY and
run-action dispatches a binding directly, so a test built on either
would pass against a debounce hung on any clock at all. Driving the
keyboard through the OS is what makes the assertion mean something.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm release \
  /c/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/tests/herdr_e2e.rs
```

---

## What this plan does not do

The reselect bug from section 5 of the spec needs no alacritree change. Its cause is herdr's headless server not projecting `Method::AgentFocus` to attached clients, fixed upstream by `985d3442` and carried by the build now installed. There is no task for it here.

The shared-view path keeps no hold-off. It ships today without one and is in daily use; extending it there is a small change and a separate decision.

## Unresolved questions

1. The quiet gap and the expiry, 750 ms and 10 s, are derived rather than measured. The poll interval already puts up to two seconds of age on a change before it is ever seen, which is the floor both sit on. Task 8 will say whether five seconds of typing is comfortably inside the expiry; a real day of use is what would say whether 750 ms is the right pause.
2. Whether reselect recovers against the server now running. If it does not, the spec's section 5 diagnosis is wrong and that item comes back onto the board.
3. Whether the `egui::Event` list in Task 7 matches the variants this workspace's egui actually exposes. A renamed variant compiles and silently narrows the clock.
