## Problem Statement

An alacritree session dies with alacritree. A crash, an update or a quit ends every shell and every agent that runs in it. The herdr and zellij backends keep terminals alive, but each one has quirks: herdr serves one request per connection, so each WSL stream costs its own `wsl.exe` launch, and its attach mode has had bugs. Running `hc attach` inside a native session feels like a native app, but it loses Ctrl+\ to detach, and the process probe sees `hc attach` instead of the program in the terminal.

herdr-core (`hc`) keeps terminals alive in a core, and it speaks a framed protocol that carries many terminals over one connection. alacritree cannot use that today, on Windows or in WSL.

## Solution

alacritree gets a session backend choice. With the `herdr_core` session backend, each new session runs as a terminal that a core owns, on the session's side. alacritree speaks the framed protocol itself, so the terminal behaves like a native session: keys, resize, focus, the foreground process and its cwd all work, and nothing runs `hc attach` inside a PTY.

Sessions outlive alacritree. Closing a session kills its terminal. Quitting detaches, and the next launch adopts every terminal that alacritree's labels mark, into the same workspace and position. On WSL, one resident connection per distro carries every stream, so a session costs no `wsl.exe` launch, and a keeper holds the distro open after alacritree quits.

alacritree also lists terminals that it did not create as panes, so the user can attach to them or watch them. Every session reports the agent in its foreground, on either backend and on every side, so the sub-agents of Claude Code and Codex appear under their session, WSL sessions included. When a side has no usable core, the session opens native and a notice says why.

## User Stories

1. As a user, I want to choose `herdr_core` as my session backend in config, so that my sessions survive an alacritree crash or quit.
2. As a user, I want `native` to stay the default session backend, so that nothing changes until I opt in.
3. As a user, I want all herdr-core settings in one `[integrations.herdr_core]` section, so that I find them in one place.
4. As a user, I want to open one session on the other backend from the palette, so that I can use a native session for a task that needs sixel.
5. As an IPC client, I want `create_session` to take an optional session backend, so that a script or an agent can pick the backend per session.
6. As a user, I want a `herdr_core` session to accept keys, paste, mouse and resize like a native session, so that I cannot tell the two apart while I work.
7. As a user, I want Ctrl+\ to reach the program in a `herdr_core` session, so that no key is lost to a detach chord.
8. As a user, I want the sidebar and the tab title to show the program that runs in a `herdr_core` session and its cwd, so that the session looks the same as a native one.
9. As a user, I want closing a `herdr_core` session to kill its terminal, so that close means the same thing on both backends.
10. As a user, I want quitting alacritree to leave my `herdr_core` terminals running, so that my agents keep working.
11. As a user, I want the next launch to put each of my terminals back in its workspace and position, so that I continue where I stopped.
12. As a user, I want my workspace layout to come back with fresh shells after a WSL shutdown or a reboot lost the programs, so that I do not rebuild my tabs by hand.
13. As a user, I want a program that exits, for example a shell after Ctrl-D, to close its session, so that a finished program behaves the same as on the native backend.
14. As a user, I want alacritree to remove the record of a finished terminal that it created, so that `hc ls --all` does not fill with old records.
15. As an agent, I want to put a terminal into an alacritree workspace with `hc run --keep --label alacritree.workspace=<path>`, so that the user sees it without any action.
16. As a user, I want alacritree to adopt such a terminal without moving my focus, so that an agent never takes my keyboard.
17. As a user, I want `hc ls` and the nested `hc` picker to show the project and checkout of each terminal that alacritree created, so that I can tell them apart from a shell.
18. As a user, I want terminals that alacritree did not create listed as panes in the sidebar, so that I can reach terminals that herdr-app or an agent started.
19. As a user, I want to open a foreign pane and type into it, so that I can work in a terminal that another client started.
20. As a user, I want to watch a foreign pane without typing into it or resizing it, so that I can follow an agent without disturbing it.
21. As a user, I want closing a session on a foreign pane to only detach, so that I never kill a terminal that I did not create by accident.
22. As a user, I want a "Kill terminal" action on a foreign pane, from the palette and from its sidebar row, with a confirmation, so that I can end a stuck program on purpose.
23. As a user, I want a foreign terminal's record to stay after its program ends, so that its creator decides what happens to it.
24. As a user, I want to list foreign panes or not, with a config switch, so that the sidebar shows only what I use.
25. As a user, I want the sub-agents of a Claude Code or Codex session listed as indented rows under that session, so that I see what my agent started.
26. As a user, I want to turn the sub-agent listing off per harness in config, so that a busy agent does not crowd the sidebar.
27. As a user, I want sub-agents listed for native sessions and for herdr-core sessions alike, so that the session backend does not change what I see.
28. As a WSL user, I want sub-agents listed for my WSL sessions, so that agents in a distro show the same detail as agents on Windows.
29. As a user, I want `hc` and `hc attach` typed inside a session to work as they do in any terminal, so that the hc command line stays usable.
30. As a WSL user, I want `herdr_core` sessions in each distro, so that my agents in WSL survive too.
31. As a WSL user, I want a new session in a distro to cost no `wsl.exe` launch once that distro is connected, so that a session opens as fast as a native one.
32. As a WSL user, I want the sidebar to fill from the WSL helper while the resident connection opens, so that a cold start does not leave the sidebar empty.
33. As a WSL user, I want my distro's terminals to survive alacritree quitting, so that WSL's idle shutdown does not kill my agents.
34. As a WSL user, I want the keeper to exit when its core has no terminals left, so that a distro does not stay up for nothing.
35. As a WSL user, I want `doctor` to explain WSL's idle shutdown and `instanceIdleTimeout`, so that I can choose a global setting myself.
36. As a user, I want alacritree to find each core's socket the same way `hc` does, so that alacritree and my `hc` command line reach the same core.
37. As a user whose core listens at a path that `hc` cannot find, I want a popup that offers to pin the live path in hc's config, so that I fix the mismatch in one step.
38. As a user, I want alacritree never to restart a core that has live terminals, so that fixing a config never kills my programs.
39. As a user with `herdr_core` selected and no `hc` on a side, I want a popup that shows the official install command and runs it only after I confirm, so that I install hc without a manual search.
40. As a user, I want a session to open native, with a notice that names the cause, when the side's core cannot be used, so that a new tab always gives me a terminal.
41. As a user, I want a session to show that it is reconnecting when the connection to its core drops, so that I know the frozen screen is not a hang.
42. As a user, I want alacritree to reconnect and repaint every session after the connection returns, so that a dropped `hc connect` costs me nothing.
43. As a user, I want the repaint after an attach to restore as much scrollback as my `scrolling.history` setting asks for, so that reattached sessions keep their history.
44. As a user, I want one busy session not to slow the other sessions on the same side, so that an agent that floods output does not freeze my shell.
45. As a user, I want the Windows side to run `herdr_core` sessions through the Windows core, so that native Windows sessions survive too.
46. As a user, I want herdr-app not to adopt or kill the terminals that alacritree created, so that the two clients can share one core.
47. As a user, I want the herdr backend to keep working beside the herdr-core backend, so that I keep my herdr agents.
48. As a developer, I want the session backend behind a trait with a test fake, so that app behavior is testable without a core.
49. As a developer, I want an opt-in end-to-end suite against a throwaway core, so that the hc client is tested against the real protocol without touching a live core.

## Implementation Decisions

### Config

- `[integrations.herdr_core]` holds every herdr-core setting:
  - `path` and `wsl_path` name the `hc` binary on the native side and inside WSL.
  - `icon` is the glyph on a pane's row.
  - `session_backend` is an enum, `native` or `herdr_core`, and its default is `native`.
  - `enabled` lists foreign panes, and its default is on.
- `[integrations.claude]` and `[integrations.codex]` each get a `subagents` switch, and its default is on.
- The config section lives in the backend crate, and the app's `RawIntegrations` names it with one field, as for the other integrations.
- The palette and the IPC `create_session` request take an optional session backend that overrides the config for one session.

### Modules

- A new integration-type crate holds the session backend trait, its shared models, its `thiserror` error type and a scripted test fake.
- A new native backend crate holds the code that opens a PTY and starts `alacritty_terminal`'s event loop. This code moves out of the app's session module.
- A new herdr-core backend crate holds the framed client, the demux thread, the per-session parser threads, socket discovery, the keeper and its implementation of the multiplexer trait for foreign panes.
- A new integration-type crate for sub-agents holds the source trait, its models, a test fake and the incremental readers for append-only files. The Claude Code integration crate and a new Codex integration crate implement it. These readers read the files each harness already keeps, and need nothing installed in the agent.
- The app dispatches through a `#[derive(ambassador::Delegate)]` enum over the two backends. Trait signatures name types by absolute path.

### Session backend interface

- The backend level opens a session, lists the terminals that are adoptable, attaches, watches and detaches.
- The session handle sends input, resizes, reports focus, kills, and reports its foreground program: the side, the pid on that side, the argv and the cwd.
- Kill and detach are separate operations. A detach never kills a terminal.
- The native backend takes the foreground program from the process probe on the native side, and from the WSL helper's probe in a distro. The helper's probe reports the pid as well as the name.
- The herdr-core backend takes the foreground program from the core's events, on every side.

### Sub-agents

- A sub-agent source receives each session's foreground program and reads that side's harness files. Claude Code maps an agent pid to its conversation, and Codex matches a session by the directory it started in.
- On a WSL side, the source reads the distro's harness files through the WSL helper.
- The sidebar lists each running sub-agent as an indented row under its session. A row is not a session: clicking it brings its session on screen, and the cursor, search and reordering pass over it.
- A row disappears shortly after its sub-agent finishes or is interrupted. A sub-agent that waits on its own background work stays listed until that work reports back.

### Data path for herdr-core sessions

- One `hc connect` child per side carries all streams in framed mode. On the native side alacritree spawns it directly. For each WSL distro it is one resident `wsl.exe -d <distro> -- hc connect`, opened at the same time as the WSL helper.
- One-shot calls (`terminal.list`, `terminal.run`, `terminal.kill`, labels) go through the WSL helper, so they need no new `wsl.exe` launch.
- A demux thread per connection reads frames and routes each channel's bytes into a byte channel for its session. The demux thread never takes a `Term` lock.
- A parser thread per session drains its byte channel into its `Term` under the existing `FairMutex` lease, with the same read limits, synchronized-update timeout and wakeups through `EventProxy` as the native event loop.
- The parser thread sends CREDIT for the bytes it has parsed, so hc's flow control follows the parser.
- Query replies go out as AUTOMATIC_INPUT when the core advertises it, and as DATA otherwise. RESIZE, FOCUS and FACTS go out when they change.
- CLOSE ends the session: a program exit closes it, as on the native backend.
- No `EventedPty` implementation exists for herdr-core streams. See the ADR on the demux thread.

### Lifetime and ownership

- alacritree creates its terminals with `keep: true`.
- alacritree's terminals carry only `alacritree.*` labels. These labels record ownership, the workspace and the position. They never carry `herdr.*` labels, so herdr-app neither adopts nor kills them.
- alacritree writes a `where` label of the form `<project> / <checkout>` on the terminals it creates.
- Adoption runs on every listing and every event. A live terminal with an `alacritree.workspace` label and no session goes into that workspace, at its recorded position, without a focus change. Relaunch is this rule on a full listing.
- Closing a session on an own terminal kills the terminal and removes its finished record. Closing a session on a foreign pane only detaches.
- A finished own terminal closes its session and loses its record. A finished foreign terminal closes its session and keeps its record.
- "Kill terminal" on a foreign pane asks for a confirmation, from the palette and from the sidebar row.
- A terminal that another terminal started with `hc run` gets no special treatment. It is a foreign pane like any other.

### Reconnect

- When a connection drops, its sessions show a reconnecting state. alacritree retries the connection every second.
- After the reconnect, each session resets its `Term` and attaches again with `history` equal to `scrolling.history`. The core's first paint restores the screen, the modes and the scrollback.
- alacritree drops late results from the earlier connection.

### Socket discovery

- alacritree runs plain `hc connect` and lets hc resolve the socket in its own order: `HC_SOCKET`, then the `socket` key in hc's `config.toml`, then the runtime dir.
- When hc refuses because a core listens elsewhere, alacritree reads the live path from the core's `core.lock` and test-connects to it. A popup then offers to write that path as the `socket` key in hc's `config.toml`.
- alacritree never restarts a core that has live terminals. A restart onto a new socket path loses every terminal, because the holder socket moves with the core socket.

### WSL keeper

- On quit, alacritree leaves one detached, hidden `wsl.exe -d <distro>` process for each distro whose core has live terminals.
- The keeper exits when its core has no live terminals.
- `doctor` explains WSL's idle shutdown and the global `instanceIdleTimeout` setting. See the ADR on the keeper.

### Fallback and install

- When a side's core cannot be used, a new session opens native, and a notice names the cause.
- When `session_backend` is `herdr_core` and `hc` is missing on a side, a popup shows the official install command for that side. alacritree runs the command only after the user confirms.

### Limits

- herdr-core sessions have no sixel. The core does not advertise sixel and cannot parse it, and alacritree does not work around this.
- alacritree ignores herdr-app's layout documents.

## Testing Decisions

- A good test drives behavior that the user or an IPC client can see: a session opens, closes, survives a quit, gets adopted, or falls back. It does not assert on threads, channels or frame bytes.
- **Seam 1: the session backend trait, through the app.** The trait crate ships a scripted fake. App tests use it to cover close kills, quit detaches, label-driven adoption, foreign-pane close detaches, the kill confirmation, the fallback to native with a notice, the per-create override, and sub-agent rows placed by the foreground program that the fake reports. Prior art: `alacritree_multiplexer::Scripted` and the app tests that drive multiplexers through it.
- **Seam 2: the herdr-core backend against a throwaway core.** A new `hc_e2e` suite joins the opt-in `e2e` task. It isolates its core with its own `HC_SOCKET` in a temporary directory and its own `HC_SESSION`, so it never touches a live core. It covers the attach and the first paint, CREDIT, a reconnect after the `hc connect` child is killed, `keep`, labels, `where`, the foreground program and socket discovery through `core.lock`. Prior art: `herdr_e2e` and `zellij_e2e`.
- The sub-agent readers get fixture tests over recorded harness files: both transcript formats, the incremental fold, and a sub-agent that waits on its own background work. Prior art: the readers in the unmerged sub-agent PR.
- The keeper and the WSL helper path get manual verification, because WSL's idle shutdown needs a real distro.
- The config keys pass the `config_schema` test, as every new key does.

## Out of Scope

- Splits. A later spec adds them, with a per-side core document named `alacritree` that holds the split tree under compare-and-swap.
- Remote servers and hc's machine federation. An existing ticket tracks remote server support.
- Sixel in herdr-core sessions.
- herdr-app's layout documents and its `herdr.seen` and `herdr.pinned` labels.
- Feature requests or changes to herdr-core.
- A palette picker for terminals that no session shows. Foreign panes in the sidebar and the nested `hc` picker cover this.
- Intercepting `hc` or `hc attach` typed inside a session.
- A hand-off verb that turns the warm WSL helper into the hc connection.

## Further Notes

- herdr does not run on herdr-core. herdr panes and hc terminals are separate, so the herdr backend stays as it is.
- The implementation must verify three facts before it relies on them:
  - the keeper's exact command, and how it learns that its core has no live terminals;
  - whether the Windows core needs herdr-app's workaround for ConPTY, which merges a bare Escape with the next key and drops Shift+Enter;
  - whether the shell that `keep` starts in place of a lost program opens in the old cwd.
- The sub-agent work takes the readers and the sidebar rows from alacritree/alacritree#407, which has not merged. It replaces that PR's process-tree walk with the foreground program that each session backend reports, and it adds WSL.
- Terms: a side is the native host or one WSL distro. A session backend is a native PTY or a core. A core is a running herdr-core server on one side. A pane is a terminal that alacritree lists without having opened it. A keeper holds a WSL distro open after alacritree quits. To adopt is to take a labelled terminal into a session. To watch is to attach without typing or resizing. A sub-agent is an agent that a session's agent starts inside its own harness.
