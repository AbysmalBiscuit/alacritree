# alacritree

alacritree is a terminal that hosts sessions behind a sidebar of projects and their checkouts.

## Language

**Side**:
The machine or WSL distro that a session, a pane or a core runs on: the native host, or one named WSL distro.
_Avoid_: location, host, target

**Session backend**:
What a session's terminal runs on: a native PTY that alacritree owns, or a terminal that a core owns.
_Avoid_: session type, terminal backend

**Pane**:
A terminal that a multiplexer or a core owns and alacritree lists in the sidebar for attaching, without having opened it.
_Avoid_: terminal, agent row

**Core**:
A running herdr-core server on one side, which owns terminals that outlive the clients attached to them.
_Avoid_: hc server, daemon, herdr server

**Keeper**:
A process that alacritree leaves behind when it quits, which holds a WSL distro open while that distro's core still has terminals.
_Avoid_: keepalive, holder
