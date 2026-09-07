# herdr integration design

**Goal:** agents running under a herdr server appear in alacritree's left sidebar under the worktree they are working in, with live status, and Enter opens one in an ordinary alacritree session.

**Issue:** [#67](https://github.com/AbysmalBiscuit/alacritree/issues/67). Parent [#19](https://github.com/AbysmalBiscuit/alacritree/issues/19). Live status without polling is split out as [#69](https://github.com/AbysmalBiscuit/alacritree/issues/69).

**Branch:** `feat/herdr-integration`. Cut from the open PR carrying the highest `[n]` marker, which was PR 210 (`fix/wsl-helper-liveness`, marker `[8]`) when this was written. Read the tip fresh at setup time rather than trusting that number.

**Base branch matters here.** `alacritree/src/jobs.rs` exists in PR 210's tip and not on `master`. Section 3 depends on it. A reader checking these claims against `master` will not find the pool.

**Platform:** all four. Native Windows lists agents but cannot attach to one directly; see section 6.

**Config:** new `[ui.herdr]` table, `enabled` default `true`. Default-on matches `doppler.rs` and `pr_status.rs`, which carry no enable key at all and simply no-op when their binary is absent. Section 7 covers what makes default-on cheap.

**herdr version:** 0.8.2, registered with `docm` and pinned to `v0.8.2` (commit `9eb5214`). Every herdr citation below is a path under that checkout. Wire protocol 20 on the WSL server, 22 on the native Windows one, observed simultaneously on one machine.

## Context

herdr is a terminal workspace manager for coding agents. It runs a server that owns PTYs, detects which agent is running in each pane, and tracks that agent's state. alacritree already detects agents itself, from `comm`/`cmdline` plus Braille spinners in the terminal title, but only for PTYs it owns. An agent running under herdr is invisible to it.

The integration is one-way and read-mostly: alacritree asks a herdr server what agents it has, renders them, and can hand one to a shell. herdr is never told about alacritree's own sessions, and herdr never owns an alacritree PTY. That boundary is what keeps this a few hundred lines instead of a second session backend, and the parent issue records the session-backend idea as rejected.

### What #67 got wrong

The issue was written from reading herdr's source at an earlier version. Verified against 0.8.2 as installed:

| #67 claims | Actually |
| --- | --- |
| `herdr terminal attach <terminal_id>` | No `terminal` subcommand. Attach is `herdr agent attach <target>` |
| `herdr agent list --json` | `agent list` takes no options at all; its output is already JSON. Other subcommands do take `--json` (`session list`, `status`, `api schema`) |
| `events.subscribe` gives live state over the CLI | No CLI verb reaches it. Nearest is `herdr agent wait --until`, one blocked process per agent |
| Live state arrives "for free" | It arrives by polling, or by writing a socket client (#69) |
| `blocked` is one of three live states | `AgentStatus` has five: `idle`, `working`, `blocked`, `done`, `unknown` |
| Windows attach is undocumented-but-maybe | Hard `#[cfg(windows)]` refusal, `src/client/mod.rs:940-947` |
| Attach is a private view of one agent | Exclusive, and it resizes the agent. Section 6 |

The five-value status enum feeds back into [#66](https://github.com/AbysmalBiscuit/alacritree/issues/66), whose three-value live axis was written from the same wrong reading.

One thing is better than claimed: `AgentInfo` carries an `agent_session` field holding a resume reference (`source`, `agent`, `kind`, `value`). Nothing populates it on either agent observed here, so this design reads it but does not depend on it.

### The evidence base

`herdr api schema --json` prints the full wire schema, roughly 273 KB, including `AgentInfo`, `AgentStatus`, `EventKind` and the request-method list. That file is the reference for every field named below; regenerate it rather than trusting this document's transcription:

```sh
herdr api schema --json > schema.json
jq -r '.schemas.success_response["$defs"].AgentInfo' schema.json
```

Two agents per side, both servers running, projected through `jq` to the fields this design uses:

```json
{"agent":"claude","agent_status":"idle","pane_id":"w5:p1","cwd":"C:\\Users\\Lev\\Git\\github\\alacritree"}
{"agent":"codex", "agent_status":"idle","pane_id":"w4:p1","cwd":"/home/lev/Git/lev/devkit","foreground_cwd":"/home/lev/Git/lev/devkit"}
```

**That is a projection, not raw output.** herdr serializes with `skip_serializing_if`, so on Windows `foreground_cwd`, `name`, `display_agent` and `agent_session` are *absent from the JSON* rather than present as `null`. Harmless for a serde `Option`, but it decides what the section 8 fixtures must look like.

Three facts in that sample drive the design. `foreground_cwd` is absent on Windows and populated on Linux, so matching cannot depend on it. `name` is absent on every observed agent, so `pane_id` is the attach target. And the two sides spell paths differently, which is the same problem `wsl.rs` already solves for the sidebar.

## 1. Discovery and the side model

A machine can host more than one herdr server, and they cannot see each other. herdr's IPC is a local socket — AF_UNIX on Unix, a named pipe on Windows — and neither crosses the WSL2 VM boundary. On the development machine a WSL server at protocol 20 and a native Windows server at protocol 22 run at the same time, holding different agents. Which side an agent lives on is therefore a property of the agent, not an ambient fact.

```rust
/// Which herdr server an agent belongs to.  Two servers on one machine
/// cannot see each other, so this is part of an agent's identity.
pub enum Side {
    Native,
    /// Named distro, as `wsl.exe -d` spells it.
    Wsl(String),
}

/// A herdr server alacritree can reach.
pub struct Endpoint {
    pub side: Side,
    /// False on native Windows, where `herdr agent attach` refuses.
    pub can_attach: bool,
}
```

`can_attach` is `false` exactly when `side == Native && cfg!(windows)`. It is a field rather than a recomputed predicate so the sidebar and the palette cannot disagree about it.

### Reaching herdr inside WSL needs a login shell

`wsl.exe -e herdr agent list` fails: `execvpe(herdr) failed: No such file or directory`. herdr installs to `~/.local/bin`, which reaches PATH only through a login shell. Every WSL invocation therefore goes through one:

| Side | Command |
| --- | --- |
| `Native` | `herdr <args>` |
| `Wsl(d)` | `wsl.exe -d <d> --exec sh -lc 'herdr <args>'` |

The same hole exists inside the resident helper. Its `RUN` branch executes `sh -c "$script"` (`wsl_helper.rs:194`), not a login shell, which is why the helper resolves git, delta and `gh` once at hello time through `"$s" -lc 'command -v …'` (`wsl_helper.rs:150`) and carries the results as capability flags. herdr joins that hello: `command -v herdr` resolved once, and polls then invoke the absolute path rather than paying a login shell per call.

Every child spawns through `command_ext::CommandExt::hide_console`, because alacritree is a GUI-subsystem binary and a bare `Command` flashes a console window on Windows.

## 2. Data source

Poll `herdr agent list`, not `herdr api snapshot`.

`agent list` returns `AgentInfo[]` carrying every field this design uses: `terminal_id`, `pane_id`, `agent`, `display_agent`, `agent_status`, `cwd`, `foreground_cwd`, `agent_session`, `state_change_seq`, `revision`. `api snapshot` returns the same agents plus panes, tabs, workspaces, layout rectangles and split trees, all of which would be parsed and discarded. Same round trip, more surface to break when herdr bumps its protocol.

### Success is on stdout, errors are on stderr

This is the detail most likely to be got wrong, because the obvious implementation reads stdout and finds nothing.

`print_response` (`cli.rs:738-746`) and `main.rs:563-569` `println!` success and `eprintln!` errors. So `herdr agent get nosuchagent` leaves stdout **empty** and writes `{"error":{"code":"agent_not_found",…}}` to stderr, exiting 1. `server_not_running` behaves the same way. A poller that captures only stdout can distinguish success from failure by exit code but can never read `error.code`.

Capture both streams. Exit status stays the primary signal — 0 on success, 1 for both `server_not_running` and an unknown target — and stderr supplies the code that decides whether to stay quiet.

Through the WSL helper this needs one more thing: the `RUN` branch discards stderr outright (`2>/dev/null`, `wsl_helper.rs:194`), so a helper-routed poll sees only the exit code. The herdr script must redirect `2>&1` itself for the error code to survive the trip.

### Parse what is there, ignore the protocol number

Nothing reads herdr's protocol version, and an unrecognised one does not disable an endpoint. Parsing is best-effort by field: deserialise the fields this design names, ignore every field it does not, and drop an individual agent only when a field it cannot work without — `terminal_id`, `pane_id`, `agent_status` — is missing or unparseable.

The alternative, gating on a known protocol number, fails in the direction that costs the user something. The two servers on the development machine already report 20 and 22, so a version allowlist would have to be widened on every herdr release, and a herdr that upgrades under a running alacritree would blank the sidebar rather than degrade. Additive changes — herdr's usual kind — cost a best-effort parser nothing, and a genuinely incompatible change surfaces as agents that fail to parse and get logged, which is the same signal without the collateral.

`agent_status` is the one field to treat generously: an unrecognised status string maps to `unknown` rather than failing the agent, so a sixth `AgentStatus` value ships as a plain-looking row instead of an empty sidebar.

## 3. Polling, backoff and the job pool

A throttle per endpoint shaped like `git_status.rs`'s `StatusCache`: a `pending: Option<Pending>` (`git_status.rs:158`) that makes a tick with a poll already in flight a no-op rather than a second spawn.

Work runs on the pool in `jobs.rs`, which arrived with the base branch. Its `Blocking` token is the point: the helpers that block take one, only a pool worker is handed one, so calling such a helper from `update` does not compile. herdr polling has no business on the UI thread and the type system should be what says so.

### Backoff is two-tier, not permanent

A single permanent disable is wrong in one common case: `herdr update` restarts the server, and every later poll would find an endpoint alacritree had already given up on.

- An endpoint that has **never** answered — no binary, or no server since startup — is disabled for the process lifetime. This is what makes `enabled = true` cheap: a user without herdr pays one failed spawn, not one per tick.
- An endpoint that **was** reachable and stops answering retries on a slow timer, around 30 s, indefinitely. A restarted server reappears on its own.

Errors other than `server_not_running` are logged once per distinct message rather than once per tick, so a protocol mismatch after `herdr update` produces one line, not one every two seconds.

### WSL discovery is bounded by the helper, and that is visible

`wsl_helper::client()` (`wsl_helper.rs:526-547`) spawns lazily on the first git call for a WSL worktree and returns `None` while starting. Its registry is private, so "distros with a live helper" cannot be queried, and a probe run at startup would find nothing and — under a permanent-disable rule — kill the WSL endpoint before any helper existed.

So the herdr probe is not scheduled by alacritree at all. It rides the helper's hello, alongside the existing `command -v` capability resolution (section 1), and runs when a helper *becomes ready*.

The consequence has to be stated rather than buried: **a WSL herdr server is only visible once some WSL worktree is registered in alacritree**, because nothing else starts a helper. A user whose projects are all on the Windows side will not see their WSL agents. Enumerating distros to fix this would drag `wsl.exe` spawns onto startup for everyone, which is the trade this design declines.

`distros()` is a `OnceLock` (`wsl.rs:198-208`), so it does not re-read the registry per frame; [#38](https://github.com/AbysmalBiscuit/alacritree/issues/38) is about its empty result never filling the cache. Neither fact changes the decision above, but the earlier draft of this spec cited #38 wrongly and the correction belongs on the record.

### What polling cannot see

`state_change_seq` increments on every status change, so a poll can detect that transitions happened. It cannot recover what they were. Measured by prompting a live Claude Code agent with a one-word reply and sampling at 0.7 s:

```
t+01  idle     seq=1
t+02  working  seq=2
t+03  working  seq=2
t+04  idle     seq=3
```

The `working` phase lasted roughly 1.4 s. At a 1.5 s throttle that turn renders as `idle` throughout. The default interval is therefore 2 s as a compromise rather than a fix, and #69 exists because the fix is a subscription. Reproduce the measurement before arguing about the interval:

```sh
herdr agent prompt <target> 'reply with exactly the word pong and nothing else'
# sample: herdr agent list | jq -r '.result.agents[0] | "\(.agent_status) \(.state_change_seq)"'
```

## 4. Workspace matching

Match `foreground_cwd` when present, else `cwd`. Windows omits the former, so a design that required it would work on Linux and silently match nothing on Windows.

WSL agents report Linux paths. Translate with `wsl::linux_to_windows(linux, distro)` (`wsl.rs:93`), which also maps `/mnt/c/…` back to a drive path, so `/home/lev/Git/lev/devkit` becomes the `\\wsl.localhost\<distro>\…` spelling `normalize_root` (`wsl.rs:126`) already canonicalises project roots into. The reverse, `windows_to_linux` (`wsl.rs:147`), is not needed here.

Match by longest path prefix, component-wise. `rebase_scope` in `doppler.rs:99` already demonstrates why component-wise matters: a string prefix lets `/repo-other` match `/repo`. On Windows the comparison is case-insensitive, because herdr reports the cwd as the shell spelled it and `Path::starts_with` is case-sensitive there — an agent started from `c:\users\lev\…` would otherwise match nothing.

**Unmatched agents go under Home, not into the bin.** Home is the `WorkspaceKey::None` tab and is already the catch-all for sessions with no worktree. Unmatched is the common case rather than the edge: of the four agents observed during design, the WSL pair sat in `/home/lev/Git/lev/devkit`, a repository that is not an alacritree project at all. Dropping them would have shown an empty herdr section on a machine running four agents.

## 5. Sidebar rows

`SidebarRow` (`sidebar_nav.rs:16`) gains one variant:

```rust
/// A herdr-managed agent, keyed by the server it lives on and the terminal
/// it runs in.  Both parts are needed: terminal ids are only unique within
/// one server.
HerdrAgent(Side, TerminalId),
```

**The key is `terminal_id`, not `pane_id`.** `pane_id` is positional, `w<N>:p<M>`, and a pane moved to another workspace gets a new one; `NEXT_WORKSPACE_ID` is a process static (`workspace.rs:107`) that restarts at `w1` after `session delete`, so pane ids are reused across server restarts. `terminal_id` is `term_{micros:x}{counter:x}` (`src/terminal/id.rs:21`), is `required` in `AgentInfo`, and survives a pane move. Attach still *targets* `pane_id`, because `agent attach` resolves pane ids and agent names only (`src/app/terminal_targets.rs:75-100`) — the identity and the target are simply different fields.

Rows sort after a workspace's own session rows, so alacritree's real sessions always come first and herdr's are visibly secondary. They obey the same listing rule as session rows (`ListedSessions`, threshold and config overrides), so a workspace that hides its single session row does not sprout a herdr one.

Row content: the agent glyph for `agent`/`display_agent`, the status, and a `herdr` marker distinguishing it from a real session. alacritree recognises six agent names (`AGENT_PROCESS_NAMES`, `session.rs:215`) while herdr detects many more — copilot, devin, droid, grok, hermes, kilo, kimi, opencode, pi, qwen and others. An unrecognised name renders a generic agent glyph rather than none, so an unknown agent still reads as an agent.

Status rendering deliberately does not wait for [#66](https://github.com/AbysmalBiscuit/alacritree/issues/66). herdr hands us a status directly, so these rows can render one before alacritree's own state model grows to five values. When #66 lands, these rows adopt it; until then they render herdr's string.

### One agent, one row

An attached agent does not get two rows. Attaching replaces the `HerdrAgent` row with the ordinary session row for the shell running the attach command; detaching brings the `HerdrAgent` row back. The sidebar therefore shows each agent exactly once whatever its state, and the row you are looking at is always the one you can act on.

The two states are distinguished by weight rather than by position. A detached `HerdrAgent` row draws in `theme.text_dim` (`app.rs:80`, computed at `:153`), the same treatment `worktree_gone` (`app.rs:1510`) already gives a row whose checkout is missing: present, listed, not currently live. An attached agent is an ordinary session row at full strength, because that is what it is.

**The attachment key lives on the `Session`, not in a side map.** A `HashMap<HerdrKey, SessionId>` in `app.rs` would need an entry removed on every path that can end a session, and `reap_exited_sessions` (`app.rs:6953`) is only one of them — a detach, a `close_session`, and a `ChildExit` from herdr's own side all reach it. Storing `Option<HerdrKey>` on the `Session` makes the association die with the session automatically, so a stale entry resurrecting a phantom attached row is not a bug that can be written.

That field is not free, and the plan should budget for it. `Session::spawn` (`session.rs:1056-1064`) and `spawn_session_with_shell` (`app.rs:1111-1117`) take fixed argument lists and return a `SessionId`, so the key is either set after the call returns or plumbed through the way `wsl_probe` already is. `visible_rows` (`sidebar_nav.rs:42`) grows a second input beside `ListedSessions`: the set of keys currently claimed by a live session, which is what the row filter reads.

### Keeping the reconcile cheap

The earlier draft of this spec described this wrongly, in a way that would have sent an implementer at the wrong function.

`visible_rows` and `capture` do **not** run every frame. The per-frame path is `ObservedInputs::matches` (`sidebar_focus.rs:447`, called from `app.rs`), which compares cheap observed inputs and rebuilds the snapshot only on a difference. So the goal is not "make `visible_rows` allocation-free"; it is to make herdr state an observed input.

Follow the `pr_generation` pattern (`sidebar_focus.rs:310`, compared at `:457`): a `herdr_generation: u64` bumped when a poll produces a result that differs in a **rendered** field — workspace, key, status, agent name, attached-or-not. Bumping on `revision` or `state_change_seq` would rebuild the snapshot every two seconds for agents that look identical on screen, which is exactly the cost this avoids. Every `UiInputs` literal in `steady_state.rs` gains the field; that churn is expected and is what keeps the assertion honest.

## 6. Attach

Enter spawns an ordinary session through `spawn_session_with_shell` (`app.rs:1111`) whose program is the attach command. Nothing in the grid, renderer, selection or input path changes: from alacritree's side this is a shell running a program.

Two side effects of that function apply and should not be a surprise at implementation time: it refuses the spawn when `worktree_gone(dir)`, and it runs `sync_doppler_scopes(dir)` first. The refusal means a herdr agent whose matched worktree has been removed cannot be attached from that dimmed row; such agents fall back to Home, matching the unmatched rule in section 4.

### Attach is exclusive, and it resizes the agent

`herdr agent attach` is not a passive view, and #67 described it as one.

`attach_terminal_client` (`src/server/headless.rs:2848-2856`) refuses a second attach to the same terminal without `--takeover`, replying with a shutdown message. On success it records the terminal in `direct_attach_resize_locks` and calls `runtime.resize(rows, cols, …)` (`:2890-2899`), so the agent's PTY takes the attaching pane's dimensions and is pinned there until detach.

Two consequences for the UI. Attaching from alacritree reflows the agent inside the human's own herdr window, which is startling if unannounced. And a second attach — a second alacritree window, or the same agent opened twice — fails rather than duplicating, which is the correct behaviour but only if the failure is visible (below). Detach is `Ctrl+B q` (`src/client/mod.rs:137-144`) and the row hint should say so, because nothing else in alacritree uses that key.

**`can_attach: true` — Linux, macOS, and any WSL distro.**

```
herdr agent attach <pane_id>
```

**`can_attach: false` — native Windows.**

```
herdr agent focus <pane_id>
herdr session attach <session_name>
```

`run_terminal_attach` has two definitions in `src/client/mod.rs`: the `#[cfg(unix)]` one at :930 does the work, and the `#[cfg(windows)]` one at :941 returns `Unsupported` unconditionally. It is a compile-time refusal with no flag or environment variable behind it, so nothing alacritree does can reach the good path on Windows.

`herdr session attach` does work on Windows, verified by running it and watching the herdr UI stream as ANSI. The session name comes from `herdr session list --json`, which reports `default` on both sides here; it is not assumed.

The cost is larger than "two rows share a view", and the spec says so plainly because the user will notice. herdr's focus is server-global — `SessionSnapshot` carries a single `focused_pane_id` and the server a single `foreground_client_id` (`headless.rs:3050-3051`) — and a newly connected app client becomes the foreground one, with `effective_size` following it (`headless.rs:1195`, `:1213`) and foreground thereafter following whichever app client was last active (`src/server/clients.rs:262-268`). So attaching from an alacritree pane resizes the human's entire herdr window to that pane's size, and typing on either side flips it back. That is what the shared-view marker in section 5 warns about.

### A failed attach must not vanish

"Detach needs no code" also means "a failed attach needs no code", and that is a bug. An attach refused for any reason — `already has an attached client`, `agent_not_found`, the Windows `Unsupported` — exits within a frame, `reap_exited_sessions` (`app.rs:6953`) closes the session, and the child's stderr dies with it. The user sees a pane flash and nothing else.

A session holding a herdr attachment key therefore does not auto-reap on a non-zero exit. It stays open showing the child's output, exactly as a shell that exited with an error would, and the row returns to its detached state only once the user closes it. A zero exit — an ordinary detach — reaps as before and restores the dimmed row with no interaction.

### The escape hatch, deliberately not taken

`run_terminal_session_observe` and `run_terminal_session_control` (`src/client/mod.rs:950`, `:958`) are not cfg-gated, so herdr's NDJSON bridge — base64 ANSI frames out, `terminal.input` commands in — works on Windows today. Driving it means a pump program translating between that stream and a PTY, which is most of what this design exists to avoid. It is the eventual route to real per-agent Windows attach without waiting for herdr to ship one, and it belongs in its own issue after this lands.

## 7. Config

A new `[ui.herdr]` table. alacritree-only options live under `[ui]` per `AGENTS.md`, which is reason enough; the earlier draft cited `[ui.wsl]` as precedent, but that table holds a single deprecated key (`config.rs:1839-1844`) and the live one is top-level `[wsl]`, so it is not a precedent worth leaning on.

| Key | Default | Meaning |
| --- | --- | --- |
| `enabled` | `true` | Discover herdr servers and show their agents |
| `poll_interval_ms` | `2000` | How often a reachable endpoint is re-polled |
| `show_unmatched` | `true` | Show agents whose cwd matches no workspace, under Home |

`poll_interval_ms` follows the existing spelling for durations, which are all `_ms: Option<u64>` deserialised into a `Duration` at build time — `attention_grace_ms` (`config.rs:1948`, built at `:2203`) and `blink_interval` (`:1511`).

Default-on is safe because the feature is inert without herdr: a never-reachable endpoint is disabled after one failed probe (section 3). Someone who runs herdr but does not want alacritree touching it sets `enabled = false`.

Each key needs a doc comment on its `Raw*` struct in `config.rs`, because those comments are the hover text the published JSON Schema carries. Regenerate afterwards, or the build fails on a stale schema:

```sh
ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema
```

## 8. Testing

Everything worth testing is a pure function, which is why `herdr.rs` holds no egui types — the same reason `sidebar_nav.rs` and `path_style.rs` do not.

- **Parsing.** Captured real `agent list` payloads from both a Windows and a WSL server, as fixtures. The Windows fixture must have `foreground_cwd`, `name`, `display_agent` and `agent_session` **absent** rather than `null`, because that is what `skip_serializing_if` produces; one fixture carrying explicit `null` covers the shape the schema also permits.
- **Stream routing.** A `server_not_running` fixture read from stderr with stdout empty and exit 1, since a parser that only reads stdout passes every other test and fails in production.
- **Best-effort parsing.** A payload carrying an unknown top-level field parses; an agent missing `terminal_id` is dropped without taking its siblings with it.
- **Status mapping.** All five `AgentStatus` values, plus an unrecognised string mapping to `unknown` rather than failing the agent.
- **Path translation and matching.** Linux path through `linux_to_windows` to a workspace; component-wise prefix so a sibling with a shared string prefix does not match; case-insensitive comparison on Windows; unmatched falling through to Home.
- **Command construction.** Per side, that `Wsl` wraps in `sh -lc`, and that `can_attach: false` produces the focus-then-session-attach pair rather than `agent attach`.
- **Row replacement.** An agent with a live attached session yields no `HerdrAgent` row; the same agent with no session yields one. A pure function over an agent list plus the set of claimed keys.
- **Generation counter.** A poll differing only in `revision`/`state_change_seq` does not bump `herdr_generation`; one differing in status does.
- **Steady state.** `steady_state.rs` already asserts the unchanged-frame reconcile allocates nothing; herdr rows must not break it.

No test invokes a real herdr binary. Arnaud's CI has none, and a test that silently skips when a binary is missing is a test that passes for the wrong reason.

## Out of scope

- **Live state without polling.** [#69](https://github.com/AbysmalBiscuit/alacritree/issues/69).
- **Per-agent Windows attach through the NDJSON bridge.** Section 6; needs its own issue.
- **`agent_session` resume references.** Read and carried, but nothing populates the field on either observed agent, so no behaviour depends on it.
- **`alacritree-session.py`.** Lives in the chezmoi dotfiles repo. Its `cmd_record` keys on `ALACRITREE_SESSION_ID`, which a herdr-attached agent does not carry, so save/restore of these sessions is broken until that script learns about `HERDR_PANE_ID`. Tracked in #67's own description.
- **Writing to herdr.** No prompts, no key sends, no pane creation. `agent focus` is the single exception, and only as a step of attaching.

## Settled during design

Recorded so they are not reopened from the same starting assumptions.

**`poll_interval_ms` stays at 2000.** It will miss turns as short as the one measured in section 3, and that is accepted for v1 rather than tuned blind. #69 is the fix; a lower default would trade a real per-tick process cost on every machine for a partial improvement on one symptom.

**One agent, one row.** Attaching replaces the herdr row with the session row, detaching brings it back, and a detached row draws dimmed. Section 5 covers the mechanism and why the attachment key lives on the `Session`.

**Best-effort parsing, no protocol gate.** Section 2. Unrecognised fields are ignored, an unrecognised `agent_status` maps to `unknown`, and no version number can blank the sidebar.

## Open questions

None outstanding.
