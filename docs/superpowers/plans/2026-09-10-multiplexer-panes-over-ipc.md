# Multiplexer panes over IPC Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a client list every herdr pane alacritree has detected, attach a session to one, and create a new pane and land in it, the last reachable from a key binding and the command palette.

**Architecture:** Three new `IpcRequest` variants, three `NamedAction`s, and a `Multiplexer` enum whose trait carries the questions alacritree asks whichever multiplexer owns a pane. Every one of them ends at `attach_herdr_agent`, the single function the sidebar click, the sidebar keyboard path and the command palette already share, so an attach asked for from outside the window behaves exactly like a click, `attach = "session"` and native Windows included. The listing reads the endpoint caches the sidebar draws from. Attach and create replies are deferred until the attached session's PTY is live, the contract `create_session` already keeps, because both attach paths finish frames after the request arrives: a direct attach waits on a PTY opened on a worker, and a shared-view attach waits on two herdr processes before a session record even exists.

**Tech Stack:** Rust 2024, serde_json, clap, the existing `ipc`/`mcp`/`cli` trio, the `herdr` CLI.

## Global Constraints

- Build, test, lint and format through devkit, never by typing cargo: `devkit run task fmt` (nightly rustfmt), `devkit run task check`, `devkit run task clippy`, `devkit run task test` (nextest). `devkit run task <name>` forwards no trailing arguments.
- **Never disable the rustc cache via an env var prefix.** Caching works correctly on this system.
- Every file this plan touches is already claimed under this session's `lockm` identity. Do not call `lockm acquire` or `lockm release`.
- Comments explain the *why*, never the *what*. Timeless and standalone: no PR/issue/task references, no `this PR` / `now we` / `used to`, no RED/GREEN narration, no forward references, no pointers to this plan. Never add a rote restatement of the line below it. Never delete an existing comment unless the change makes it wrong.
- Conventional Commits. Imperative subject under 72 chars, no trailing period, lowercase after the colon. Body wrapped at 72 columns, saying what was wrong and why this approach. One logical change per commit; stage selectively and read `git diff --staged` before committing.
- Every commit ends with a `Co-Authored-By: <your model name> <noreply@anthropic.com>` trailer naming the model that wrote it.
- Prose outside commit messages (Markdown docs, doc comments) is soft-wrapped: one line per paragraph, one line per bullet. Do not hard-wrap it at a column.
- `rg` for content, `fd` for filenames. `find` and `grep` are refused by a hook. `rg` recurses by default and `-r` means `--replace`.
- Preserve existing behavior. Everything here is additive: nothing changes what an unmodified config does, and no existing key binding is reassigned.
- Use `git -C <absolute path>` rather than changing directory first. Never use `git stash`.
- alacritree is a GUI-subsystem binary: spawn every child process through `command_ext`, never `std::process::Command` directly.

## Reference: what herdr answers

`herdr` prints a JSON envelope on stdout and errors on stderr, and its field set grows between releases, so every type reading it tolerates unknown and missing fields. `herdr tab create --focus --cwd <path>` answers:

```json
{"id":"cli:tab:create","result":{"type":"tab_created","tab":{"tab_id":"w_1:2","workspace_id":"w_1","number":2,"label":"review","focused":true,"pane_count":1,"agent_status":"unknown"},"root_pane":{"pane_id":"w_1-3","terminal_id":"term_example","workspace_id":"w_1","tab_id":"w_1:2","focused":true,"cwd":"/tmp/review","agent_status":"unknown"}}}
```

`root_pane.terminal_id` is the identity alacritree keys a session on, and `root_pane.pane_id` is what an attach targets. `--cwd` takes a value, `--focus` takes none.

---

### Task 1: The pane listing

**Files:**
- Modify: `alacritree/src/herdr/model.rs`
- Modify: `alacritree/src/ipc.rs`
- Modify: `alacritree/src/app.rs`
- Modify: `alacritree/src/cli/mod.rs`
- Modify: `alacritree/src/cli/offline.rs`
- Modify: `alacritree/src/cli/render.rs`
- Modify: `alacritree/src/mcp.rs`
- Modify: `docs/alacritree.md`
- Test: `alacritree/src/herdr/model.rs`, `alacritree/src/cli/render.rs`, `alacritree/src/app.rs`

**Interfaces:**
- Produces: `IpcRequest::ListMultiplexerPanes`; `herdr::Side::name() -> String`; the free function `multiplexer_json(side, terminal_id, session, pane) -> Value` in `app.rs`; `AlacritreeApp::multiplexer_panes_json()`.
- Consumed by Task 2, which parses a side back out of `Side::name`'s output and resolves a pane this listing named.

**The working tree already carries part of this task.** `git -C <worktree> diff` shows uncommitted edits to `model.rs`, `ipc.rs`, `app.rs`, `render.rs` and `offline.rs` written against these requirements, which pass `devkit run task check`. Read them first, check them against the steps below, correct anything that disagrees, then finish the rest and commit the whole thing as one change. `cli/mod.rs` and `mcp.rs` are untouched.

- [ ] **Step 1: Give a side a name that leaves the process**

`herdr::Side` already has `label()` in `herdr/cli.rs`, which returns `None` for the native side because a sidebar row on a machine with only that side would carry the same word on every row. That is a row's word and not an identity, so the wire cannot use it. Add a second spelling to `alacritree/src/herdr/model.rs`, directly after the `Side` enum:

```rust
impl Side {
    /// How a side is spelled outside the process: `native`, or `wsl:<distro>`
    /// as `wsl.exe -d` names it.  Two herdr servers on one machine cannot see
    /// each other, so a pane named to a client without its side is not named
    /// at all.
    pub fn name(&self) -> String {
        match self {
            Self::Native => "native".to_string(),
            Self::Wsl(distro) => format!("wsl:{distro}"),
        }
    }
}
```

`app.rs` has a private `side_label` free function producing exactly these two strings for `session_json`. Delete it and call `key.side.name()` in its place, so one function decides how a side is spelled.

- [ ] **Step 2: Add the request**

In `alacritree/src/ipc.rs`, add a variant to `IpcRequest` immediately before `RunAction`:

```rust
    /// Every pane the multiplexer integration has detected, whether or not a
    /// session is attached to one.  A caller reaches an unattached pane no
    /// other way: `ListSessions` describes only what alacritree already
    /// holds.
    ListMultiplexerPanes,
```

and its arm in `IpcRequest::name`, keeping that match in the same order as the enum:

```rust
            Self::ListMultiplexerPanes => "ListMultiplexerPanes",
```

- [ ] **Step 3: Build the reply**

In `alacritree/src/app.rs`, `session_json` calls a method `multiplexer_json(&self, key)` that looks a pane up by key. A listing already holds the `Agent`, so the shape moves to a free function and the method becomes a by-key lookup over it. Replace the existing `multiplexer_json` method with:

```rust
    /// Where a herdr-backed session lives, looked up from the key the session
    /// carries.
    fn session_multiplexer_json(&self, key: &herdr::HerdrKey) -> Value {
        multiplexer_json(
            &key.side,
            &key.terminal_id,
            self.herdr_session_name(&key.side),
            self.find_herdr_agent(&key.side, &key.terminal_id),
        )
    }

    /// Every pane herdr reports, on every side, attached or not.  Unlike the
    /// sidebar this hides nothing: `show_unmatched` decides what is worth
    /// drawing, and a caller naming a pane by its id is not browsing.
    fn multiplexer_panes_json(&self) -> Value {
        if !self.config.integrations.herdr.enabled {
            return json!({ "panes": [] });
        }
        let workspaces = herdr_workspaces(&self.projects, |path| self.liveness.missing(path));
        let mut panes = Vec::new();
        for cache in self.herdr_endpoints.caches() {
            let side = cache.side();
            let session = cache.session_name();
            for agent in cache.agents() {
                let key =
                    herdr::HerdrKey { side: side.clone(), terminal_id: agent.terminal_id.clone() };
                panes.push(json!({
                    "multiplexer": multiplexer_json(
                        side,
                        &agent.terminal_id,
                        session.clone(),
                        Some(agent),
                    ),
                    "kind": agent.kind,
                    "title": agent.title,
                    "status": agent.status.map(|status| status.label()),
                    "focused": agent.focused,
                    "workspace": herdr::match_workspace(agent, side, &workspaces),
                    "session_id": self.herdr_session_for(&key),
                }));
            }
        }
        json!({ "panes": panes })
    }
```

and point `session_json` at the renamed method:

```rust
            "multiplexer": key.map(|key| self.session_multiplexer_json(key)),
```

Add the free function beside `activity_json`, which sits below the `impl` block:

```rust
/// Where a herdr pane lives, in the fields an attach takes back.  `pane_id`
/// and `tab_id` are null when the listing does not carry the pane, which says
/// the cache does not know right now rather than that the pane is gone.
fn multiplexer_json(
    side: &herdr::Side,
    terminal_id: &str,
    session: Option<String>,
    pane: Option<&herdr::Agent>,
) -> Value {
    json!({
        "name": "herdr",
        "side": side.name(),
        "session": session,
        "terminal_id": terminal_id,
        "pane_id": pane.map(|pane| pane.pane_id.clone()),
        "tab_id": pane.and_then(|pane| pane.tab_id.clone()),
    })
}
```

Add the dispatch arm in `handle_ipc_request`, immediately before the `Req::RunAction` arm:

```rust
            Req::ListMultiplexerPanes => Ok(self.multiplexer_panes_json()),
```

- [ ] **Step 4: Refuse it with no window**

A detected pane exists only because a running alacritree is polling for it, so `alacritree/src/cli/offline.rs` cannot serve this. Add `IpcRequest::ListMultiplexerPanes` to the `or`-pattern whose arm is `Err("alacritree is not running".to_string())`.

- [ ] **Step 5: Render it**

In `alacritree/src/cli/render.rs`, add the arm right after the `ListSessions` one:

```rust
        IpcRequest::ListMultiplexerPanes => multiplexer_panes(value),
```

and the function, above `fn screen`:

```rust
fn multiplexer_panes(value: &Value) {
    let panes = array(&value["panes"]);
    if panes.is_empty() {
        println!("no multiplexer panes");
        return;
    }
    for p in panes {
        // Side and terminal id lead because together they are what an attach
        // takes; the rest of the line is there to recognise the pane.
        let held = if p["session_id"].is_null() { " " } else { "*" };
        let name = p["title"].as_str().or_else(|| p["kind"].as_str()).unwrap_or("");
        let status = match p["status"].as_str() {
            Some(status) => format!("  [{status}]"),
            None => String::new(),
        };
        let workspace = p["workspace"].as_str().unwrap_or("home");
        println!(
            "{held} {}  {}  {name}{status}  {workspace}",
            text(&p["multiplexer"]["side"]),
            text(&p["multiplexer"]["terminal_id"])
        );
    }
}
```

- [ ] **Step 6: Add the CLI command**

In `alacritree/src/cli/mod.rs`, add to `enum Command`, before `Workspace`:

```rust
    /// Panes of the terminal multiplexer alacritree has detected.  Needs a
    /// running alacritree.
    Multiplexer {
        #[command(subcommand)]
        command: MultiplexerCommand,
    },
```

Add the subcommand enum before `enum WorkspaceCommand`:

```rust
#[derive(Debug, Subcommand)]
enum MultiplexerCommand {
    /// List every detected pane, attached or not.
    List,
}
```

And the arm in `to_request`, before the `Command::Workspace` arm:

```rust
        Command::Multiplexer { command } => match command {
            MultiplexerCommand::List => IpcRequest::ListMultiplexerPanes,
        },
```

- [ ] **Step 7: Add the MCP tool**

In `alacritree/src/mcp.rs`, `tool_definitions` lists tools in the order a client sees them. Add this entry immediately before the `select_workspace` one:

```rust
        {
            "name": "list_multiplexer_panes",
            "description": "List every pane of the terminal multiplexer alacritree has detected (herdr), on every side it reaches, whether or not a session is attached to one. Each entry carries the multiplexer block naming the pane (side and terminal_id), the pane's agent kind, title and live status, whether the multiplexer's own window is showing it, the workspace its working directory matches, and the id of the alacritree session holding it (null when none is). Unattached panes appear here and nowhere else.",
            "inputSchema": { "type": "object", "properties": {} },
        },
```

- [ ] **Step 8: Write the tests**

In `alacritree/src/herdr/model.rs`, inside the existing `mod tests`:

```rust
    /// A side has two spellings and they are not interchangeable: `label` is
    /// a row's word for it and stays silent on the native side, while a
    /// client that cannot see the row needs the side named every time.
    #[test]
    fn a_side_names_itself_on_both_sides_of_the_wire() {
        assert_eq!(Side::Native.name(), "native");
        assert_eq!(Side::Wsl("Ubuntu-24.04".into()).name(), "wsl:Ubuntu-24.04");
    }
```

In `alacritree/src/cli/render.rs`, inside `mod tests`, add `IpcRequest::ListMultiplexerPanes` to the array `absent_fields_do_not_panic` iterates, and add:

```rust
    /// A pane nothing is attached to is the whole reason for the listing, and
    /// it carries neither a session id nor a status.
    #[test]
    fn a_pane_line_survives_an_unattached_pane_with_no_agent() {
        human(
            &IpcRequest::ListMultiplexerPanes,
            &serde_json::json!({
                "panes": [{
                    "multiplexer": { "name": "herdr", "side": "wsl:ubuntu", "terminal_id": "t1" },
                    "kind": serde_json::Value::Null,
                    "title": serde_json::Value::Null,
                    "status": serde_json::Value::Null,
                    "focused": false,
                    "workspace": serde_json::Value::Null,
                    "session_id": serde_json::Value::Null,
                }]
            }),
        );
    }
```

In `alacritree/src/app.rs`, inside `mod tests`. `test_app()` and `bind_herdr_fixture` already exist there, and `herdr::Endpoints::caches_mut_for_test` is how a test populates an endpoint cache. Read those and the existing herdr tests around them before writing these three, and write real bodies:

```rust
    /// The listing is the only way a client learns a pane exists before
    /// anything is attached to it, so a pane no session holds must appear
    /// with a null session id rather than be filtered out the way the
    /// sidebar filters one.
    #[test]
    fn the_pane_listing_carries_an_unattached_pane_with_a_null_session_id() {}

    /// A pane a session already holds still appears, naming that session, so
    /// a client can tell "already open" from "not there".
    #[test]
    fn the_pane_listing_names_the_session_holding_a_pane() {}

    /// A cache nothing polls has nothing to report, so the reply is empty
    /// rather than an error: there is no pane to fail to find.
    #[test]
    fn the_pane_listing_is_empty_while_the_integration_is_disabled() {}
```

If the existing fixtures cannot populate an endpoint cache, say so in your report rather than deleting a test.

- [ ] **Step 9: Document it**

In `docs/alacritree.md`, add a row to the MCP tools table after the `list_sessions` row:

```
| `list_multiplexer_panes` | Every herdr pane alacritree has detected, attached or not, with the side and terminal id an attach takes |
```

- [ ] **Step 10: Verify**

```sh
devkit run task fmt
devkit run task clippy
devkit run task test
```

All three must pass. Clippy must be clean on the files this task touches; diff it against this task's base commit, never against your own amended commits.

- [ ] **Step 11: Commit**

Subject: `feat(ipc): list the multiplexer panes herdr has detected`. Body: a client could read what alacritree already held but not what it could still attach to, so an unattached pane was reachable only by clicking it; the reply reads the same endpoint caches the sidebar draws from and names each pane by the side and terminal id an attach takes.

---

### Task 2: The attach request

**Files:**
- Modify: `alacritree/src/herdr/model.rs`
- Modify: `alacritree/src/ipc.rs`
- Modify: `alacritree/src/app.rs`
- Modify: `alacritree/src/cli/mod.rs`
- Modify: `alacritree/src/cli/offline.rs`
- Modify: `alacritree/src/cli/render.rs`
- Modify: `alacritree/src/mcp.rs`
- Modify: `docs/alacritree.md`
- Test: `alacritree/src/herdr/model.rs`, `alacritree/src/app.rs`

**Interfaces:**
- Consumes: `Side::name` and the listing from Task 1.
- Produces: `herdr::Side::parse(&str) -> Option<Side>`; `IpcRequest::AttachMultiplexerPane`; `attach_herdr_agent`'s sixth parameter `waiter: Option<mpsc::Sender<ipc::IpcResult>>`; `open_herdr_session` returning `Option<SessionId>`; `AlacritreeApp::park_attach_reply`. Task 4 calls `attach_herdr_agent` with a waiter and reuses `park_attach_reply`.

- [ ] **Step 1: Read the three attach paths first**

Before writing anything, read `attach_herdr_agent`, `open_herdr_session` and `poll_herdr_attach` in `alacritree/src/app.rs`. They resolve three ways and this task must answer a client in all three:

1. A session already holds the key. `activate_session_by_id` runs and that session is live now.
2. `herdr_attaches_directly(&key)` is true. `open_herdr_session` spawns the attach client and a session record exists before the call returns, but its PTY may still be opening on a worker.
3. Otherwise it is a shared-view attach. A `PendingHerdrAttach` is queued, two herdr processes run off the UI thread, and `poll_herdr_attach` opens the session frames later. Nothing exists to answer with when the request returns.

Also read `defer_create_session` and `pending_spawn.rs`. `PendingSpawns::watch` parks a reply channel until a session's PTY is live and hands the channel back when nothing is opening for that id, and `poll_pending_spawns` answers a parked waiter with `Ok(json!({ "session_id": id }))`. That is the contract to match: a client that attaches in order to read the pane would otherwise race the PTY, the same reason `create_session` defers.

- [ ] **Step 2: Read a side back off the wire**

In `alacritree/src/herdr/model.rs`, add to `impl Side`, after `name`:

```rust
    /// Read back what `name` wrote.  A `wsl:` with nothing after it names no
    /// server, so it is refused rather than resolving to a distro called the
    /// empty string.
    pub fn parse(name: &str) -> Option<Self> {
        if name == "native" {
            return Some(Self::Native);
        }
        match name.strip_prefix("wsl:") {
            None | Some("") => None,
            Some(distro) => Some(Self::Wsl(distro.to_string())),
        }
    }
```

- [ ] **Step 3: Add the request**

In `alacritree/src/ipc.rs`, directly after `ListMultiplexerPanes`:

```rust
    /// Open a session on a detected multiplexer pane, the way clicking its
    /// sidebar row does.  `side` and `terminal_id` are what
    /// `ListMultiplexerPanes` reports.  The pane id is deliberately not the
    /// target: it is positional and changes when a pane moves.
    AttachMultiplexerPane {
        side: String,
        terminal_id: String,
    },
```

and its arm in `IpcRequest::name`:

```rust
            Self::AttachMultiplexerPane { .. } => "AttachMultiplexerPane",
```

- [ ] **Step 4: Take a waiter through the attach**

In `alacritree/src/app.rs`, give `PendingHerdrAttach` a field:

```rust
    /// Clients parked on this attach.  A shared-view attach opens its session
    /// frames after the request that asked for it, so there is nothing to
    /// answer with until `poll_herdr_attach` resolves.
    waiters: Vec<mpsc::Sender<ipc::IpcResult>>,
```

Change `open_herdr_session` to return `Option<SessionId>` rather than `bool`: the `Ok(id)` arm ends `Some(id)` after its existing body, and the `Err` arm keeps its `error_dialog` assignment and returns `None`. Update both call sites to test `.is_some()`.

Give `attach_herdr_agent` a sixth parameter, `waiter: Option<mpsc::Sender<ipc::IpcResult>>`, and answer it at each of the three resolutions:

- Session already open: send `Ok(json!({ "session_id": id }))` at once, since that session's PTY is live.
- Direct attach: park the waiter with the helper below on the id `open_herdr_session` returned, or send `Err("failed to attach the pane".to_string())` when it returned `None`.
- Shared view: push the waiter onto the new `PendingHerdrAttach`. Where the function returns early because an attach for this key is already in flight, extend *that* entry's `waiters` instead, so a second client joins the first one's attach rather than being dropped.

Add the helper beside `attach_herdr_agent`:

```rust
    /// Answer an attach once the session's PTY is live.  A client that
    /// attached in order to read the pane would otherwise be handed an id
    /// before anything behind it can answer.
    fn park_attach_reply(&mut self, id: SessionId, waiter: Option<mpsc::Sender<ipc::IpcResult>>) {
        let Some(waiter) = waiter else { return };
        if let Some(waiter) = self.pending_spawns.watch(id, waiter) {
            let _ = waiter.send(Ok(json!({ "session_id": id })));
        }
    }
```

The three existing callers pass `None`.

In `poll_herdr_attach`, every arm that ends a pending attach now owes its waiters an answer:

- `Some(Ok(...))` where `open_herdr_session` returns `Some(id)`: park the waiters on that id.
- `Some(Ok(...))` returning `None`, `Some(Err(e))`, and the `job.failed()` arm: send each waiter an `Err` carrying the message its error dialog gets.

`pending.waiters` has to come out of the entry before `pending.key` and `pending.workspace` are moved into `open_herdr_session`, so take them first with `std::mem::take`.

- [ ] **Step 5: Serve the request**

The reply channel is needed inside the handler, so this request is claimed in `process_ipc_calls` alongside `CreateSession` rather than reaching `handle_ipc_request` with a return value:

```rust
                ipc::IpcRequest::AttachMultiplexerPane { side, terminal_id } => {
                    self.defer_attach_multiplexer_pane(ctx, &side, &terminal_id, reply_tx);
                    continue;
                },
```

and `handle_ipc_request` gets the arm that says so, spelled the way `CreateSession`'s is:

```rust
            // Claimed by `process_ipc_calls` before dispatch: the reply is
            // held until the attached session's PTY is live, which needs the
            // reply channel this method does not have.
            Req::AttachMultiplexerPane { .. } => {
                Err("attach_multiplexer_pane was not deferred".to_string())
            },
```

Write the deferred handler beside `defer_create_session`:

```rust
    /// Attach to a pane the way its sidebar row does, holding the reply until
    /// the session behind it can be read.  A refusal names what went wrong
    /// precisely enough to act on: a side that names no server, a pane no
    /// endpoint is reporting, and an integration that is switched off are
    /// three different situations, and only the last is worth retrying after
    /// a config change.
    fn defer_attach_multiplexer_pane(
        &mut self,
        ctx: &Context,
        side: &str,
        terminal_id: &str,
        reply_tx: mpsc::Sender<ipc::IpcResult>,
    ) {
```

Its body, in order. Every refusal sends its message on `reply_tx` and returns, the way `defer_create_session` sends its own failures rather than returning them:

1. `!self.config.integrations.herdr.enabled` refuses with `"the herdr integration is disabled ([integrations.herdr] enabled)"`.
2. `herdr::Side::parse(side)` refuses with `format!("`{side}` is not a side, expected `native` or `wsl:<distro>`")`.
3. `self.find_herdr_agent(&parsed_side, terminal_id)` refuses with `format!("no pane `{terminal_id}` on {side}, see list_multiplexer_panes")`. Clone `pane_id` off the agent before the borrow ends.
4. Build the `herdr::HerdrKey`.
5. Resolve the workspace the session opens in with `herdr::match_workspace`, against `herdr_workspaces(&self.projects, |path| self.liveness.missing(path))`. That is the call `multiplexer_panes_json` makes, so the listing's `workspace` field is what the attach honours.
6. Switch and restore exactly as the palette's `AttachHerdrAgent` arm does:

```rust
        let previous = std::mem::replace(&mut self.current_workspace, workspace.clone());
        if !self.attach_herdr_agent(ctx, key, &pane_id, workspace, previous.clone(), Some(reply_tx))
        {
            self.current_workspace = previous;
        }
```

- [ ] **Step 6: Refuse it with no window**

Add `IpcRequest::AttachMultiplexerPane { .. }` to the `"alacritree is not running"` pattern in `alacritree/src/cli/offline.rs`.

- [ ] **Step 7: Render it**

In `alacritree/src/cli/render.rs`:

```rust
        IpcRequest::AttachMultiplexerPane { .. } => {
            println!("session {}", text(&value["session_id"]));
        },
```

- [ ] **Step 8: Add the CLI command**

In `alacritree/src/cli/mod.rs`, add to `MultiplexerCommand`:

```rust
    /// Open a session on a detected pane, as clicking its sidebar row does.
    Attach {
        /// `native`, or `wsl:<distro>`, as `multiplexer list` reports it.
        side: String,
        /// Terminal id from `multiplexer list`.  Not the pane id, which
        /// changes when a pane moves between workspaces.
        terminal_id: String,
    },
```

and the arm in `to_request`:

```rust
            MultiplexerCommand::Attach { side, terminal_id } => {
                IpcRequest::AttachMultiplexerPane { side, terminal_id }
            },
```

Leave `timeout_for` alone in both `cli/mod.rs` and `mcp.rs`. The app's own `APP_REPLY_TIMEOUT` of 10s bounds this request from the server side and expires first, which puts the message in the hands of the side that knows why it overran, the same reasoning `IPC_CREATE_BUDGET` carries in `ipc.rs`.

- [ ] **Step 9: Add the MCP tool**

In `alacritree/src/mcp.rs`, after the `list_multiplexer_panes` entry:

```rust
        {
            "name": "attach_multiplexer_pane",
            "description": "Open an alacritree session on a multiplexer pane from list_multiplexer_panes, exactly as clicking its sidebar row does, and return the session id once the session can be read. A pane a session already holds returns that session rather than opening a second one. side and terminal_id both come from the pane's multiplexer block; the pane id is not a target, since it changes when a pane moves.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "side": { "type": "string", "description": "\"native\" or \"wsl:<distro>\", from list_multiplexer_panes." },
                    "terminal_id": { "type": "string", "description": "Terminal id from list_multiplexer_panes." },
                },
                "required": ["side", "terminal_id"],
            },
        },
```

- [ ] **Step 10: Write the tests**

In `alacritree/src/herdr/model.rs`, inside `mod tests`:

```rust
    /// A client names a pane by the side it read out of a listing, so a side
    /// that does not survive the round trip points an attach at the wrong
    /// server or at none.
    #[test]
    fn a_side_reads_back_as_the_side_it_spelled() {
        for side in [Side::Native, Side::Wsl("Ubuntu-24.04".into())] {
            assert_eq!(Side::parse(&side.name()), Some(side));
        }
    }

    #[test]
    fn a_side_that_names_no_server_is_refused() {
        for name in ["", "wsl", "wsl:", "Native", "tmux:0"] {
            assert_eq!(Side::parse(name), None, "{name} named a server");
        }
    }
```

In `alacritree/src/app.rs`, inside `mod tests`. Each of these drives `defer_attach_multiplexer_pane` with its own `mpsc::channel` and reads the reply off the receiver, so each asserts on the message text rather than only that an `Err` arrived. Use `test_app`, `bind_herdr_fixture` and the endpoint-cache fixture Task 1 established. No test here may spawn a real PTY or a real herdr process. Write real bodies:

```rust
    /// Three refusals a client acts on differently: only the disabled one is
    /// worth retrying after a config change, and a caller that cannot tell
    /// them apart retries all three or none.
    #[test]
    fn attaching_to_a_side_that_names_no_server_is_refused_by_name() {}

    #[test]
    fn attaching_to_a_pane_no_endpoint_reports_is_refused_by_name() {}

    #[test]
    fn attaching_while_the_integration_is_disabled_says_so() {}

    /// A pane a session already holds answers with that session rather than
    /// opening a second attach client against the same pane.
    #[test]
    fn attaching_to_a_pane_a_session_already_holds_returns_that_session() {}
```

- [ ] **Step 11: Document it**

In `docs/alacritree.md`, add the MCP tools table row after `list_multiplexer_panes`:

```
| `attach_multiplexer_pane` | Open a session on a detected herdr pane, as clicking its sidebar row does |
```

And in the `### herdr agents` section, extend the **Attaching** bullet with a sentence saying the same attach is reachable from outside the window as `alacritree multiplexer attach <side> <terminal-id>` and as the `attach_multiplexer_pane` MCP tool, that both name the pane by the side and terminal id `multiplexer list` reports, and that the reply waits until the session can be read. Keep it soft-wrapped as one line, in the voice of the bullets around it.

- [ ] **Step 12: Verify**

```sh
devkit run task fmt
devkit run task clippy
devkit run task test
```

- [ ] **Step 13: Commit**

Subject: `feat(ipc): attach a session to a detected multiplexer pane`. Body: an unattached pane could be listed but only opened by clicking it; the request resolves a side and terminal id back to the key the sidebar uses and hands it to the same attach, and the reply is held until the session's PTY is live because both attach paths finish frames after the request arrives.

---

### Task 3: One seam for every multiplexer

**Files:**
- Create: `alacritree/src/multiplexer.rs`
- Modify: `alacritree/Cargo.toml`
- Modify: `alacritree/src/main.rs` (the module list)
- Modify: `alacritree/src/herdr/mod.rs`, `alacritree/src/herdr/cli.rs`, `alacritree/src/herdr/model.rs`
- Modify: `alacritree/src/app.rs`
- Test: `alacritree/src/multiplexer.rs`

**Interfaces:**
- Consumes: `attach_herdr_agent` and `open_herdr_session` as Task 2 left them.
- Produces: `multiplexer::Multiplexer`, the trait `multiplexer::MultiplexerSession`, `multiplexer::Side`, `multiplexer::PaneTarget`, `multiplexer::Launch`, and the unit struct `multiplexer::Herdr`. Task 4 adds a method to the trait; Task 5 reaches it through the enum.

**This task changes no behavior.** It moves the two questions alacritree asks herdr behind a trait, so that a second multiplexer becomes a new enum variant rather than a new branch in `app.rs`. Every existing test must pass unchanged. A test that needs editing to keep passing means the move was not behavior-neutral: stop and report rather than editing the test.

**What the seam covers, and what it deliberately does not.** alacritree asks herdr two questions a second multiplexer would answer differently: how to open a session on a pane it can hand over, and what to run first when it can only share its whole view. Those two go behind the trait. Polling, the endpoint caches, the follow-focus state machine in `herdr/view.rs`, the detach chord and indicator settings, and the sidebar's row model all stay herdr-shaped, because their shape is guesswork until a second multiplexer exists to disagree with them. Do not abstract them.

- [ ] **Step 1: Verify the derives before designing around them**

`enum_dispatch` requires each enum variant to be a newtype over a type implementing the trait, so the enum is `Herdr(Herdr)` rather than a fieldless `Herdr`. `strum`'s `Display`, `EnumString` and `EnumIter` derives behave differently on newtype variants than on fieldless ones, and this plan's author did not verify which combination compiles on the pinned versions.

Before writing anything else, put a throwaway probe in `multiplexer.rs`, compile it with `devkit run task check`, and find out. If a strum derive refuses a newtype variant or produces the wrong string, hand-write `std::fmt::Display` and `std::str::FromStr` for `Multiplexer`, and keep `EnumIter` only if it compiles. Two hand-written three-line functions are a better outcome than a derive fought into place.

Record in your report which derives you kept and which you hand-wrote, with the compiler's reason for each one you dropped.

The name on the wire must be exactly `herdr`, lowercase, because `multiplexer_json` already emits that string and clients read it.

- [ ] **Step 2: Add the dependencies**

In `alacritree/Cargo.toml`, alongside the existing dependencies, each with the comment style its neighbours use (a reason, not a restatement):

```toml
# One trait call in `app.rs` reaches whichever multiplexer owns the pane, so
# a second one is a new variant rather than a new branch at every call site.
# Static dispatch: the enum carries no vtable.
enum_dispatch = "0.3"
# Spells a multiplexer's name for the wire and reads it back, off the same
# enum the dispatch uses, so the two cannot disagree.
strum = { version = "0.26", features = ["derive"] }
```

`enum_dispatch 0.3.13` and `strum 0.26.3` are both already in this machine's cargo registry, so this needs no network. Do not move either past those lines.

- [ ] **Step 3: Give `Side` a home that is not herdr's**

`Side` says whether a server runs natively or inside a named WSL distro. That is true of any multiplexer, so its definition moves to `multiplexer.rs`: the enum, its `name`, its `parse`, its `label`, and its `command`.

`herdr::Side` is named in ten files and roughly a hundred and sixty places. Renaming all of them would bury this task's actual seam in mechanical churn, so **re-export it**: `pub use crate::multiplexer::Side;` in `herdr/mod.rs`, which keeps every existing `herdr::Side` path compiling. Say in the re-export's doc comment that the type belongs to the multiplexer module and herdr names it for its own callers' convenience. Migrating those call sites is deliberate follow-up, not this task's work.

`Side::command` hardcodes the string `herdr`. Give it a `program: &str` parameter and pass `"herdr"` from herdr's own call sites, so a second multiplexer runs its own binary through the same WSL and native plumbing. Find every call site with `rg`.

- [ ] **Step 4: The target and the launch**

In `multiplexer.rs`:

```rust
/// A pane alacritree wants a session on, in the terms the multiplexer that
/// owns it uses.  `terminal_id` is the identity because a pane id is
/// positional and changes when a pane moves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneTarget {
    pub side: Side,
    pub terminal_id: String,
    pub pane_id: String,
    /// The tab holding the pane, where the multiplexer reports one.
    pub tab_id: Option<String>,
    /// Whether the multiplexer reports an agent in this pane.  Some of them
    /// resolve a target through an agent registry, which holds nothing for a
    /// pane running a plain shell, so such a pane is reached another way.
    pub has_agent: bool,
}

/// A program and its argv, ready to be a session's shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub program: String,
    pub argv: Vec<String>,
}
```

- [ ] **Step 5: The trait and the enum**

Both live in `multiplexer.rs`, together with the `Herdr` unit struct implementing the trait, so `enum_dispatch`'s macros never have to see across a module boundary. `herdr/` stays the implementation detail it already is, reached from here through its existing free functions.

```rust
/// What a multiplexer answers so alacritree can host one of its panes.
#[enum_dispatch]
pub trait MultiplexerSession {
    /// The command that opens a session already showing `target`, when this
    /// multiplexer can hand one pane over on this side under `mode`.  `None`
    /// means the pane is reachable only by sharing the multiplexer's whole
    /// view, which `shared_view_gesture` prepares.
    fn open_multiplexer_session(&self, target: &PaneTarget, mode: AttachMode) -> Option<Launch>;

    /// What a shared view needs before its client can start: point the
    /// multiplexer at the pane, then name the session to attach to.  Both are
    /// process calls, so this only ever runs off the UI thread.
    ///
    /// `cached_name` is the session name already learned in the background.
    /// A gesture that beats the first read asks the multiplexer itself, since
    /// a wait is better than a refusal.
    fn shared_view_gesture(
        &self,
        target: &PaneTarget,
        cached_name: Option<String>,
    ) -> Result<Launch, String>;
}

/// The terminal multiplexers alacritree can host a pane from.
#[enum_dispatch(MultiplexerSession)]
pub enum Multiplexer {
    Herdr,
}
```

with whichever `strum` derives Step 1 proved out, and `Herdr` a unit struct deriving what those derives require of it.

- [ ] **Step 6: Implement it for herdr**

`impl MultiplexerSession for Herdr` delegates to the free functions already in `herdr/cli.rs`. Move no logic: `open_multiplexer_session` is `herdr::attaches_directly` deciding, then `herdr::attach_args` and `Side::command` building the launch; `shared_view_gesture` is `herdr::herdr_attach_gesture` with its result mapped into a `Launch`. If a delegation needs an argument the trait does not carry, say so in your report rather than widening the trait on your own judgement.

- [ ] **Step 7: Call it from `app.rs`**

`attach_herdr_agent` asks `self.herdr_attaches_directly(&key)` and then builds the argv itself, and `poll_herdr_attach` calls `herdr::herdr_attach_gesture` directly. Both become calls through a `Multiplexer` value.

Where that value comes from: a `Session`'s `herdr_key` names a herdr pane, so today the answer is always the `Herdr` variant. Write one small accessor that produces it, put it where a second multiplexer would change it, and do not thread a new field through `HerdrKey` or `Session` for a choice that has one option. Say in your report where you put it and why a second multiplexer would edit that one place.

The two call sites keep their current shape: a `Some(launch)` takes today's direct-attach branch, a `None` takes today's shared-view branch, and the gesture still runs on `jobs::pool()`. Nothing about when work runs, or on which thread, may change.

- [ ] **Step 8: Write the tests**

In `multiplexer.rs`:

```rust
    /// A client reads a multiplexer's name off a reply and may send it back,
    /// so the two spellings have to agree.  `herdr` is lowercase because that
    /// is the string already on the wire.
    #[test]
    fn a_multiplexer_reads_back_as_the_name_it_spelled() {}

    /// A pane with an agent on a side that can hand one over opens directly,
    /// and the same pane with no agent does not, because every `herdr agent`
    /// subcommand resolves through a registry holding nothing for it.
    #[test]
    fn a_pane_with_no_agent_is_never_opened_directly() {}

    /// The configured attach mode outranks capability in one direction only:
    /// asking for a shared view always gets one, and no user is handed a
    /// direct attach they did not ask for.
    #[test]
    fn asking_for_a_shared_view_never_opens_a_pane_directly() {}
```

Write real bodies. `open_multiplexer_session` is pure, so these need no herdr process and no PTY. Do not test `shared_view_gesture`, which spawns one.

If the existing tests in `herdr/cli.rs` already cover a case here, say so in your report and drop the duplicate rather than asserting the same thing twice.

- [ ] **Step 9: Verify**

```sh
devkit run task fmt
devkit run task clippy
devkit run task test
```

The suite must pass with no test edited. Report the count and compare it against the count before this task: it should grow by exactly the tests you added.

- [ ] **Step 10: Commit**

Subject: `refactor(multiplexer): dispatch session opening through a trait`. Body: every question alacritree asked a multiplexer was a direct call into herdr's own functions, so a second one meant a new branch at each call site; an enum carrying one variant per multiplexer answers them through a trait instead, and the herdr implementation delegates to the functions that already existed.

---

### Task 4: Creating a pane and landing in it

**Files:**
- Modify: `alacritree/src/herdr/cli.rs`
- Modify: `alacritree/src/herdr/wire.rs`
- Modify: `alacritree/src/herdr/mod.rs`
- Modify: `alacritree/src/ipc.rs`
- Modify: `alacritree/src/app.rs`
- Modify: `alacritree/src/cli/mod.rs`
- Modify: `alacritree/src/cli/offline.rs`
- Modify: `alacritree/src/cli/render.rs`
- Modify: `alacritree/src/mcp.rs`
- Modify: `docs/alacritree.md`
- Test: `alacritree/src/herdr/wire.rs`, `alacritree/src/herdr/cli.rs`, `alacritree/src/app.rs`

**Interfaces:**
- Consumes: everything from Tasks 1, 2 and 3, in particular `attach_herdr_agent`'s waiter parameter and `Side::parse`.
- Produces: a third method on `MultiplexerSession`, `create_pane(&self, side, cwd) -> Result<CreatedPane, String>`, which `Herdr` implements by delegating to a new `herdr::create_pane`; and `AlacritreeApp::create_multiplexer_pane(ctx, side, workspace, waiter)`, which Task 5's action calls.

Creating goes on the trait Task 3 built, as a third method beside the two that open a session, because asking a multiplexer for a new pane is exactly the kind of question a second one would answer differently. `Herdr`'s implementation delegates to a new free function in `herdr/cli.rs`, the way the other two delegate.

Creating is two steps that must not be split across frames by hand: ask herdr for a tab, then attach to the pane it names. herdr's answer carries the pane's `terminal_id` and `pane_id`, which is everything an attach needs, so the second half is `attach_herdr_agent` with no new attach logic at all.

- [ ] **Step 1: Ask herdr for a tab**

In `alacritree/src/herdr/wire.rs`, add the reply type. It sits beside `SessionList` and follows the same absent-tolerant rules as everything else in the file:

```rust
/// What `tab create` answers.  Only the two ids an attach needs are read;
/// the tab block and the pane's own metadata arrive on the next listing
/// poll like every other pane's.
#[derive(Deserialize)]
pub(super) struct CreatedTab {
    pub(super) result: CreatedTabResult,
}

#[derive(Deserialize)]
pub(super) struct CreatedTabResult {
    pub(super) root_pane: CreatedPaneIds,
}

#[derive(Deserialize)]
pub(super) struct CreatedPaneIds {
    pub(super) terminal_id: String,
    pub(super) pane_id: String,
}
```

In `alacritree/src/herdr/cli.rs`, add the runner. It mirrors `running_session_name`: same `bounded` timeout, same `WSL_UTF8`, same `command_ext::hidden`, and the same `#[allow(clippy::disallowed_methods)]` with its reason:

```rust
/// A new pane, as herdr just made it.  Both ids come back because the two
/// answer different questions: `terminal_id` is the identity a session is
/// keyed on and survives the pane moving, `pane_id` is what an attach is
/// pointed at.
pub struct CreatedPane {
    pub terminal_id: String,
    pub pane_id: String,
}

/// Opens a tab in the user's own herdr window and focuses it, so a shared
/// view attaching afterwards is already showing the pane this returns.  A
/// process spawn that waits on herdr starting up, so it only ever runs on
/// the pool.
///
/// `cwd` is spelled in the side's own terms: a Windows path on the native
/// side, and a path inside the distro on a WSL one, since herdr resolves it
/// where it runs.
pub fn create_pane(side: &Side, cwd: Option<String>) -> Result<CreatedPane, String> {
```

Its body builds `["tab", "create", "--focus"]`, appending `["--cwd", cwd]` when a cwd is given, runs it through `side.command`, captures stdout and stderr, and:

- `bounded` returning `None` is `Err("herdr did not answer while creating the pane".to_string())`.
- A spawn error is `Err(format!("failed to create a herdr pane: {e}"))`.
- A non-zero exit carries herdr's stderr verbatim, the way `focus_pane` does: `Err(format!("herdr refused to create the pane: {stderr}"))`.
- Stdout that does not parse is `Err("herdr answered with no pane".to_string())`.

Export `create_pane` and `CreatedPane` from `alacritree/src/herdr/mod.rs`'s `pub use cli::{...}` list, keeping that list alphabetical.

- [ ] **Step 2: Test the parse against a captured reply**

`wire.rs` already keeps captured herdr output in a `#[cfg(test)]` const (`PANES`). Add one beside it holding the real envelope, and a test that reads the two ids out of it:

```rust
#[cfg(test)]
/// Captured from `herdr tab create --focus`.  The tab block and the rest of
/// the pane's fields are kept as herdr sent them, so a reader that starts
/// depending on one has a real sample to read it out of.
pub(super) const CREATED_TAB: &str = r#"{"id":"cli:tab:create","result":{"type":"tab_created","tab":{"tab_id":"w_1:2","workspace_id":"w_1","number":2,"label":"review","focused":true,"pane_count":1,"agent_status":"unknown"},"root_pane":{"pane_id":"w_1-3","terminal_id":"term_example","workspace_id":"w_1","tab_id":"w_1:2","focused":true,"cwd":"/tmp/review","agent_status":"unknown"}}}"#;
```

Write two tests in `wire.rs`: one asserting both ids come out of `CREATED_TAB`, and one asserting an envelope carrying no `root_pane` fails to parse rather than yielding empty ids, since an empty pane id would be sent to herdr as a target.

- [ ] **Step 3: Run the create off the UI thread**

In `alacritree/src/app.rs`, the create is a process spawn and cannot run inline. `pending_herdr_attach` already models exactly this shape: a queued gesture, a `jobs::Job` polled once per frame, and an open when it lands. Add a sibling for the create rather than growing that struct, because a create resolves into an attach and the two queues would otherwise have to encode which stage each entry is in:

```rust
/// A herdr pane being created.  The attach it turns into is the ordinary
/// one, so this queue only carries the gesture: `poll_herdr_create` hands
/// the pane it names to `attach_herdr_agent` and stops there.
struct PendingHerdrCreate {
    job: jobs::Job<Result<herdr::CreatedPane, String>>,
    side: herdr::Side,
    workspace: WorkspaceKey,
    previous: WorkspaceKey,
    waiters: Vec<mpsc::Sender<ipc::IpcResult>>,
}
```

Add the field to `AlacritreeApp`, its `Vec::new()` to the constructor, and a `poll_herdr_create(&mut self, ctx: &Context)` called once per frame from wherever `poll_herdr_attach` is called, immediately before it, so a create that lands this frame attaches this frame.

`poll_herdr_create` polls the job and, on `Some(Ok(pane))`, builds the `HerdrKey` from the entry's side and the pane's `terminal_id`, switches `current_workspace` to the entry's workspace, and calls `attach_herdr_agent(ctx, key, &pane.pane_id, workspace, previous, waiter)` with the waiters carried through, restoring `current_workspace` on a refusal. On `Some(Err(e))` and on `job.failed()`, it restores the workspace, sets `error_dialog`, and answers every waiter with that message.

A waiter list is a `Vec` and `attach_herdr_agent` takes one `Option`, so a create with several waiters answers the extras itself: hand the first to `attach_herdr_agent` and send the rest the same reply the first receives. If that reads badly, park them all instead by having `poll_herdr_create` answer nothing and letting `attach_herdr_agent` do it, whichever you can write without duplicating the reply text. Say which you chose in your report.

Write the entry point the request and Task 5's action share:

```rust
    /// Ask herdr for a pane and open a session on it when it answers.  The
    /// two halves cannot be one call: herdr is a process, and the pane an
    /// attach needs does not exist until it answers.
    fn create_multiplexer_pane(
        &mut self,
        ctx: &Context,
        side: herdr::Side,
        workspace: WorkspaceKey,
        waiter: Option<mpsc::Sender<ipc::IpcResult>>,
    ) {
```

It resolves the cwd herdr should open in from `workspace` (a `None` workspace passes `None`, so herdr picks its own default; a `Some(path)` on a `Side::Wsl` goes through `wsl::windows_to_linux` for that distro, since herdr resolves the path where it runs), spawns the job on `jobs::pool()` at `jobs::Priority::Interactive` the way `poll_herdr_attach` spawns its gesture, pushes the `PendingHerdrCreate`, and calls `ctx.request_repaint()`.

Read `wsl.rs` for the exact translation function name and signature before using it.

- [ ] **Step 4: Add the request**

In `alacritree/src/ipc.rs`, after `AttachMultiplexerPane`:

```rust
    /// Open a new pane in the multiplexer and a session on it.  `side` and
    /// `workspace` both default: an omitted side picks the one the active
    /// session already belongs to, and an omitted workspace opens the pane
    /// in the focused one.
    CreateMultiplexerPane {
        #[serde(default)]
        side: Option<String>,
        #[serde(default)]
        workspace: Option<PathBuf>,
    },
```

and its `name` arm.

- [ ] **Step 5: Serve it**

Claim it in `process_ipc_calls` beside `AttachMultiplexerPane`, and give `handle_ipc_request` the matching "was not deferred" arm. The deferred handler:

```rust
    /// Create a pane and open a session on it.  An omitted side is the one
    /// the active session's own pane belongs to, since a user asking for
    /// another pane while looking at one means another like it; with no
    /// herdr session in front of them there is no such answer, so a machine
    /// reaching more than one server has to say which.
    fn defer_create_multiplexer_pane(
        &mut self,
        ctx: &Context,
        side: Option<&str>,
        workspace: Option<PathBuf>,
        reply_tx: mpsc::Sender<ipc::IpcResult>,
    ) {
```

In order, each refusal sent on `reply_tx`:

1. Refuse when the integration is disabled, with the message Task 2 uses.
2. Resolve the side: a given one through `Side::parse`, refused by name as in Task 2. An omitted one is `self.active_session_index()`'s session's `herdr_key`'s side when it has one. Failing that, the single endpoint cache whose side has a reachable server. Failing *that*, refuse with a message naming every side it could have meant, so the caller can retry with one. Write a small helper for this resolution, since Task 5's action needs the same answer.
3. Resolve the workspace: a given path through `self.known_worktree_path`, refused with `unknown_worktree` as `defer_create_session` does. An omitted one is `self.current_workspace.clone()`.
4. Call `create_multiplexer_pane` with the waiter.

- [ ] **Step 6: Wire the CLI, the renderer, offline and MCP**

`MultiplexerCommand` gains:

```rust
    /// Open a new pane in the multiplexer and a session on it.
    Create {
        /// `native`, or `wsl:<distro>`.  Omit to use the side the active
        /// session's pane belongs to.
        #[arg(long)]
        side: Option<String>,
        /// Worktree path; omit for the focused workspace.
        #[arg(long, value_name = "PATH")]
        workspace: Option<PathBuf>,
    },
```

with `workspace` passed through `absolute` in `to_request`, as every other path argument is. The renderer prints the session id the way `AttachMultiplexerPane` does. `offline.rs` adds it to the "alacritree is not running" pattern. The MCP tool:

```rust
        {
            "name": "create_multiplexer_pane",
            "description": "Open a new pane in the terminal multiplexer (herdr) and an alacritree session on it, and return the session id once the session can be read. Omit side to use the one the active session's pane belongs to, which is what a machine reaching only one herdr server always wants; a machine reaching several must name it when no herdr session is focused. Omit workspace to open the pane in the focused workspace.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "side": { "type": "string", "description": "\"native\" or \"wsl:<distro>\", from list_multiplexer_panes." },
                    "workspace": { "type": "string", "description": "Worktree path from list_projects; omit for the focused workspace." },
                },
            },
        },
```

- [ ] **Step 7: Write the tests**

In `alacritree/src/herdr/cli.rs`, a test that `create_pane`'s argv carries `--focus` and carries `--cwd` only when a cwd is given. Extract the argv builder into a small function so the test does not spawn herdr.

In `alacritree/src/app.rs`, tests driving `defer_create_multiplexer_pane` through a channel, asserting on the message text:

```rust
    /// Naming a side that no herdr server answers on is a different failure
    /// from naming one that is not a side at all, and a caller retrying the
    /// second is retrying a typo.
    #[test]
    fn creating_a_pane_on_a_side_that_names_no_server_is_refused_by_name() {}

    /// With no herdr session focused and more than one server reachable,
    /// there is no side the request could have meant, so the refusal names
    /// the ones it could.
    #[test]
    fn creating_a_pane_with_no_side_and_several_servers_names_the_choices() {}

    /// The side of the pane already on screen is what asking for another one
    /// means, so a focused herdr session answers the question the request
    /// left open.
    #[test]
    fn creating_a_pane_takes_the_side_of_the_focused_herdr_session() {}

    /// A worktree the sidebar does not have is refused before herdr is asked,
    /// so a typo never leaves a pane behind in the multiplexer.
    #[test]
    fn creating_a_pane_in_an_unknown_worktree_is_refused() {}
```

Write real bodies. Do not write a test that spawns herdr or a real PTY.

- [ ] **Step 8: Document it**

`docs/alacritree.md` gets the MCP tools table row:

```
| `create_multiplexer_pane` | Open a new herdr pane and a session on it |
```

and a sentence in the `### herdr agents` section saying a pane can be created from outside the window too, that the side defaults to the focused session's and the workspace to the focused one, and that the new pane appears in the sidebar under the workspace its directory matches like any other.

- [ ] **Step 9: Verify**

```sh
devkit run task fmt
devkit run task clippy
devkit run task test
```

- [ ] **Step 10: Commit**

Subject: `feat(herdr): create a pane and open a session on it`. Body: attaching could only reach a pane herdr already had, so making one meant leaving the window; the create asks herdr for a tab off the UI thread and hands the pane it names to the same attach a click takes, because herdr is a process and the pane an attach needs does not exist until it answers.

---

### Task 5: The bindable action

**Files:**
- Modify: `alacritree/src/bindings.rs`
- Modify: `alacritree/src/command_palette.rs`
- Modify: `alacritree/src/app.rs`
- Modify: `docs/alacritree.md`
- Test: `alacritree/src/bindings.rs`, `alacritree/src/command_palette.rs`, `alacritree/src/app.rs`

**Interfaces:**
- Consumes: `AlacritreeApp::create_multiplexer_pane` and the side-resolution helper from Task 4.
- Produces: nothing later tasks use. This is the last task.

One `NamedAction` reaches four surfaces at once: a `[[keyboard.bindings]]` entry, the command palette, the shortcuts window, and `alacritree action <Name>` with its `run_action` MCP tool. Nothing here adds a fifth.

- [ ] **Step 1: Add the action**

In `alacritree/src/bindings.rs`, add to `NamedAction` beside the other herdr-aware entries:

```rust
    /// Open a new pane in the multiplexer and a session on it, in the focused
    /// workspace and on the side the focused session's own pane belongs to.
    NewMultiplexerPane,
```

Then:

- `parse_action` gains `"NewMultiplexerPane" => BindingAction::Named(NewMultiplexerPane),`, placed among the neighbouring session actions rather than at the end.
- `description` gains `Self::NewMultiplexerPane => "Open a new multiplexer pane and a session on it".into(),`.
- `bindable_actions` gains the variant and its return type's array length grows by one. Read the current length rather than assuming it.
- `config_name` needs no arm: its `other => format!("{other:?}")` fallback already spells this one.

Bind no default key. Every default is already taken, and a user who wants one writes it.

- [ ] **Step 2: Put it in the palette**

In `alacritree/src/command_palette.rs`, `section_of` maps an action to its palette heading. Add `NewMultiplexerPane` to the `Sessions` arm holding `SpawnNewInstance | SpawnProfile(_) | CloseSession | CloseExitedSession`, since a herdr pane is a session the user is opening.

- [ ] **Step 3: Dispatch it**

In `alacritree/src/app.rs`, `dispatch_action` gains an arm calling `self.create_multiplexer_pane` through the same resolution `defer_create_multiplexer_pane` does, with `None` for the waiter: the side from the helper Task 4 wrote, the workspace from `self.current_workspace`. Where that helper cannot pick a side, set `error_dialog` to the message it produced rather than doing nothing, or a key press that resolves to nothing looks like a broken binding.

Read how a neighbouring herdr-touching arm in `dispatch_action` is written and match it, including whether it calls `focus_terminal` afterwards.

- [ ] **Step 4: Write the tests**

In `alacritree/src/bindings.rs`, the file already has tests asserting every bindable action parses back from its own `config_name`. Find them and confirm the new action is covered by an existing exhaustive test before writing a new one. Add one only if nothing covers it:

```rust
    /// A user binds an action by the name `config_name` prints, so an action
    /// that does not parse back from its own name is unbindable.
    #[test]
    fn the_new_pane_action_parses_back_from_its_config_name() {
        let action = NamedAction::NewMultiplexerPane;
        assert_eq!(parse_action(&action.config_name()), BindingAction::Named(action));
    }
```

In `alacritree/src/command_palette.rs`, a test that the action appears among the palette's items, following the shape of the existing tests that assert an action is present.

In `alacritree/src/app.rs`, a test that dispatching the action with no herdr server reachable leaves an error dialog rather than failing silently.

- [ ] **Step 5: Document it**

`docs/alacritree.md` lists bindable actions somewhere in the `## Input and key bindings` section. Find that list and add `NewMultiplexerPane` in the same shape as its neighbours. Extend the `### herdr agents` sentence Task 4 added with the fact that the same create is bindable as `NewMultiplexerPane` and reachable from the command palette.

- [ ] **Step 6: Verify**

```sh
devkit run task fmt
devkit run task clippy
devkit run task test
```

- [ ] **Step 7: Commit**

Subject: `feat(herdr): bind opening a new multiplexer pane`. Body: creating a pane meant sending a request, which a key cannot do; a named action reaches the same call, so the palette, the shortcuts window and a key binding all get it at once.

---

### Task 6: Attaching and detaching every pane at once

**Files:**
- Modify: `alacritree/src/bindings.rs`
- Modify: `alacritree/src/command_palette.rs`
- Modify: `alacritree/src/app.rs`
- Modify: `docs/alacritree.md`
- Test: `alacritree/src/bindings.rs`, `alacritree/src/command_palette.rs`, `alacritree/src/app.rs`

**Interfaces:**
- Consumes: `attach_herdr_agent` as Task 2 left it, `herdr_session_for`, `close_session`, and `NamedAction` as Task 5 left it.
- Produces: nothing later tasks use.

Two more actions on the surfaces Task 5 already wired: a `[[keyboard.bindings]]` entry, the command palette, the shortcuts window, and `alacritree action <Name>` with its `run_action` MCP tool. Neither gets a default key, for the same reason `NewMultiplexerPane` does not: every default is taken, and a user who wants one writes it.

Neither action gets an `IpcRequest` of its own. `alacritree action` already carries a named action over IPC, so a request would be a second spelling of a capability the CLI and the MCP bridge already reach.

- [ ] **Step 1: Add the two actions**

In `alacritree/src/bindings.rs`, beside `NewMultiplexerPane`:

```rust
    /// Open a session on every detected multiplexer pane that none is
    /// attached to yet, leaving the panes that already hold one alone.
    AttachAllMultiplexerPanes,
    /// End every session attached to a multiplexer pane.  The panes keep
    /// running under the multiplexer and their rows come back unattached.
    DetachAllMultiplexerPanes,
```

Then, exactly as Task 5 did for its own action: `parse_action` arms placed among the neighbouring session actions, `description` arms, and both variants added to `bindable_actions` with its return type's array length grown by two. Read the current length rather than assuming it. `config_name` needs no arm.

Descriptions:

```rust
    Self::AttachAllMultiplexerPanes => "Attach every unattached multiplexer pane".into(),
    Self::DetachAllMultiplexerPanes => "Detach every attached multiplexer pane".into(),
```

- [ ] **Step 2: Put them in the palette**

In `alacritree/src/command_palette.rs`, add both to the same `Sessions` arm of `section_of` that Task 5 put `NewMultiplexerPane` in.

- [ ] **Step 3: Attach every unattached pane**

In `alacritree/src/app.rs`, write one method for the attach side. It walks `self.herdr_endpoints.caches()`, and for each cache walks its agents, building the `HerdrKey` for each pane and skipping every key `herdr_session_for` already answers `Some` for. What remains goes to `attach_herdr_agent`, one call per pane, with `None` for the waiter.

Four things this must get right, none of which the call itself does for you:

**The workspace.** A pane already knows which worktree it belongs to: `herdr::match_workspace` is what the sidebar uses to file a row under one. Pass what it answers, and `self.current_workspace` where it answers nothing, so a batch attach files its sessions the same way clicking each row would. Read how the sidebar click builds its `workspace` argument and match it.

**The `previous` workspace.** `attach_herdr_agent` takes a `previous` so a refusal is readable in the workspace it happened in, and it switches `current_workspace` to `workspace` before handing the gesture over. A loop that lets each call move the workspace leaves the user somewhere they did not ask to be. Decide what `previous` should be for a batch and say so in your report. Restoring the workspace the user started in, once, after the loop, is the shape to aim for.

**Ordering and the active session.** Shared-view attaches queue on `pending_herdr_attach` and drain one at a time, because each one moves herdr's global focus. That is already true and must stay true: do not attach in parallel and do not touch the queue. Whether the last pane attached ends up the active session is a real question. Read what `open_herdr_session` and `poll_herdr_attach` do about activation and report what a batch ends up on. Do not add an activation of your own.

**A herdr that is not there.** With `[integrations.herdr] enabled = false`, or with no cache on any side, the walk finds nothing and the action does nothing. That is correct and silent. Do not set an error dialog for it: an empty listing is not a failure.

- [ ] **Step 4: Detach every attached pane**

The second method collects the id of every session whose `herdr_key` is `Some` and ends each with `close_session`. Collect the ids first: closing mutates `self.sessions`, so a loop that walks it while closing skips sessions.

`[ui] confirm_session_detach` defaults to **true** and governs the sidebar's `×`. Asking once per session would put a dialog in front of the user N times, which is unusable and is not what that setting means. Ask once for the batch instead, naming how many sessions it will detach, and go straight through when the setting is false. Read `request_close_session` and the modal it opens, and follow that shape rather than inventing a second confirmation mechanism.

A detach destroys nothing. The pane keeps running under herdr and its row comes back unattached, which `confirm_session_detach`'s own doc comment in `config.rs` already says.

- [ ] **Step 5: Dispatch them**

Two `dispatch_action` arms, following the shape of the neighbouring herdr-touching arms including whether they call `focus_terminal` afterwards.

- [ ] **Step 6: Write the tests**

In `alacritree/src/bindings.rs`, the existing exhaustive test over `bindable_actions` should already cover both new names. Confirm that it does before writing anything, and add a test only if it does not.

In `alacritree/src/command_palette.rs`, that both actions appear among the palette's items, following the existing present-in-the-palette tests.

In `alacritree/src/app.rs`, three tests, none of which may spawn a PTY or a herdr process:

```rust
    /// A pane a session already holds is not a second session's to open, or
    /// a batch attach would double every row it ran on.
    #[test]
    fn attaching_every_pane_skips_the_ones_already_attached() {}

    /// Closing walks the same list it mutates, so the ids have to be taken
    /// before the first close or the walk steps over its own removals.
    #[test]
    fn detaching_every_pane_ends_every_herdr_session_and_no_other() {}

    /// An empty listing is not a failure: a machine with no herdr running
    /// must not put a dialog in front of the user for pressing a key.
    #[test]
    fn attaching_every_pane_with_no_panes_detected_reports_nothing() {}
```

Write real bodies. `bind_herdr_fixture` and `adopt_herdr_fixture` are the existing helpers for putting herdr-keyed sessions and a cache in place without a real server; read what they do before building your own. The second test needs at least one non-herdr session present, or it cannot tell "every herdr session" from "every session".

If a test you write turns out to need a real PTY or a real herdr, say so in your report and say which assertion it was, rather than writing one that passes without reaching the code.

- [ ] **Step 7: Document them**

`docs/alacritree.md`: add both names to the bindable-actions list in the same shape as their neighbours, and extend the `### herdr agents` section with a sentence saying the whole set of detected panes can be attached or detached in one action. Say that neither carries a default key binding.

- [ ] **Step 8: Verify**

```sh
devkit run task fmt
devkit run task clippy
devkit run task test
```

Report the count and compare it against the count before this task.

- [ ] **Step 9: Commit**

Subject: `feat(herdr): bind attaching and detaching every pane`. Body: opening or ending a session per detected pane meant one click per row, which is the whole sidebar on a machine running many agents; two named actions walk the listing and the session list instead, and a detach asks once for the batch rather than once per session.

---

## What this plan does not do

Detaching has no request of its own. The `×` on a session row already ends one attach and `close_session` reaches it over IPC today, and the action that ends every attach at once travels as a named action, which `alacritree action` and the `run_action` MCP tool already carry. A request would be a second spelling either way.

The listing does not say whether attaching a given pane will attach it directly or share herdr's view. `herdr_attaches_directly` answers that from config and capability together, and a client that cannot act differently on the answer does not need to ask.

The multiplexer seam covers session opening and pane creation, and nothing else. Polling, the endpoint caches, the follow-focus state machine, the detach chord and the sidebar's row model stay herdr-shaped. Their right shape is guesswork until a second multiplexer exists to disagree with them, and a wrong abstraction there costs more than the branch it saves.

No request carries a multiplexer name. The reply says which one owns a pane, and with one implementation a client naming it could only ever name that one. The field goes in when a second multiplexer does.

Nothing here starts a herdr server. A side with no server answers the same refusal it always has, and `[integrations.herdr]` still governs whether alacritree looks at all.

## Unresolved questions

1. The app's 10s `APP_REPLY_TIMEOUT` bounds a deferred reply. A create followed by a shared-view attach on a cold WSL distro is three herdr processes and then a PTY; whether that fits in ten seconds in the worst case is not measured. Nothing cancels the work when a client gives up, so the failure mode is a confusing message and a pane that appears anyway, not a wedged session.
2. `NewMultiplexerPane` ships with no default key. Whether it deserves one, and which, is a question for a day of using it.
3. Whether the trait's two opening methods survive a second multiplexer. They are shaped by herdr's own split between handing over one pane and sharing a whole view, and a multiplexer that does neither, or does both differently, would reshape them. Nothing validates the seam until that day.
4. Whether the create should name the pane. herdr's `tab create --label` exists, and a pane named after the worktree it was opened for would read better in both windows than the default. Left out until the plain version is in use.
