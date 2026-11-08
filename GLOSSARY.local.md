# alacritree

alacritree is a terminal that hosts sessions behind a sidebar of projects and their checkouts.

## Language

**Side**:
The machine or WSL distro that a session, a foreign terminal or a core runs on: the native host, or one named WSL distro.
_Avoid_: location, host, target

**Session**:
One terminal, scratchpad, task list or diff that the sidebar lists as one row.
_Avoid_: tab, pane

**Session backend**:
What a session's terminal runs on: a native PTY that alacritree owns, or a terminal that a core owns.
_Avoid_: session type, terminal backend

**Tab**:
A group of sessions that a workspace shows on screen together, each in its own pane. A workspace shows one tab at a time and remembers which one.
_Avoid_: layout, split group, window

**Pane**:
The region of a tab that shows one session.
_Avoid_: split, surface, window

**Foreign terminal**:
A terminal that a multiplexer or a core owns and alacritree lists in the sidebar for attaching, without having opened it.
_Avoid_: pane, stray, agent row

**Adopt**:
To take a terminal that no session shows into a session, because its labels name a workspace.
_Avoid_: claim, import, reattach

**Sub-agent**:
An agent that a session's agent starts inside its own harness, such as a Claude Code `Agent` call, which the sidebar lists under that session.
_Avoid_: child agent, background agent, helper

**Watch**:
An attach to a foreign terminal that shows its output and never types into it or resizes it.
_Avoid_: view, observe

**Core**:
A running herdr-core server on one side, which owns terminals that outlive the clients attached to them.
_Avoid_: hc server, daemon, herdr server

**Keeper**:
A process that alacritree leaves behind when it quits, which holds a WSL distro open while that distro's core still has terminals.
_Avoid_: keepalive, holder
