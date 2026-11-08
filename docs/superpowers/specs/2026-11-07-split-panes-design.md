## Problem Statement

A workspace in alacritree shows one session at a time. To watch an agent while I work in a shell, or to keep a scratchpad beside a running program, I switch tabs back and forth. Every other terminal that I compare against, ghostty, kitty, wezterm, herdr and herdr-app, lets me split a tab into panes and see several sessions at once. alacritree cannot.

## Solution

A tab holds one or more sessions, each in its own pane. I split the focused pane right, left, down or up, and the new pane opens a session beside it. I move focus between panes by direction, resize them with the keyboard or by dragging a divider, zoom one pane to fill the tab, swap two panes, and equalize their sizes. Closing the last pane closes the tab.

The sidebar keeps one row per session, and a bracket joins the sessions that share a tab. The tab strip shows one entry per tab. Splits work for native sessions and for `herdr_core` sessions, and for scratchpads, task lists and diffs as well as terminals.

When a tab holds `herdr_core` sessions, alacritree mirrors its tree into a document in the core on the workspace's side. After a quit, a crash or a reboot, the next launch rebuilds each tab with its panes in place.

## User Stories

1. As a user, I want to split the focused pane to the right, so that I can see two sessions side by side.
2. As a user, I want to split the focused pane downward, so that I can stack two sessions.
3. As a user, I want to split to the left and upward from the palette, so that the new pane can open on either side.
4. As a user, I want Alt+Shift+Plus to split right and Alt+Shift+Minus to split down, so that splitting needs no mouse.
5. As a user, I want a new pane to start in the focused session's cwd and shell profile when that cwd is inside the workspace, so that I continue where I am.
6. As a user, I want a new pane to start with the workspace default when the focused session's cwd is outside the workspace or unknown, so that a pane never opens somewhere unexpected.
7. As a user, I want splitting inside a row of panes to add a sibling to that row, so that three splits give three equal columns and not a nested pair.
8. As a user, I want a split that would make a pane smaller than 10 columns or 3 rows to be refused with a toast, so that I never get a pane too small to use.
9. As a user, I want Ctrl+Alt+Arrows to move focus to the pane in that direction, so that I can reach any pane from the keyboard.
10. As a user, I want directional focus to pick the pane that overlaps on the other axis, then the nearest one, then the one I focused most recently, so that the choice matches what I see.
11. As a user, I want previous-pane and next-pane actions in the palette, so that I can cycle panes in order.
12. As a user, I want to click an unfocused pane to focus it, without the click reaching the program in it, so that a stray click never lands in an agent's TUI.
13. As a user, I want Ctrl+Alt+Shift+Arrows to move the nearest divider in that direction by 2 cells, so that I can resize without the mouse.
14. As a user, I want to drag a divider to resize the panes on each side of it, so that I can size panes by eye.
15. As a user, I want a resize to stop where a pane would fall below 10 columns or 3 rows, so that a drag cannot collapse a pane.
16. As a user, I want the divider to widen to an accent line and the cursor to change to a resize arrow on the divider's axis when I hover it, so that I can see that it can be dragged.
17. As a user, I want an equalize action in the palette, so that I can reset every pane in a tab to equal sizes.
18. As a user, I want Ctrl+Shift+Z to zoom the focused pane to fill the tab and to unzoom it, so that I can read one session at full size for a moment.
19. As a user, I want a swap action in the palette that swaps the focused pane with its neighbour in a direction, so that I can rearrange panes without closing them.
20. As a user, I want Ctrl+Shift+W to close the focused pane's session, so that close means the same as before.
21. As a user, I want the sibling of a closed pane to take its space, so that no gap stays behind.
22. As a user, I want focus to go to the pane I focused most recently after a close, so that I return to what I was doing.
23. As a user, I want the tab to close when its last pane closes, with the same landing as a tab close today, so that closing behaves the same as before.
24. As a user, I want a program that exits, for example a shell after Ctrl-D, to close its pane, so that a finished program behaves the same as in a tab.
25. As a user, I want the focused pane to show a thin accent outline when its tab has more than one pane, so that I always know where my keys go.
26. As a user, I want unfocused panes to keep full brightness, so that I can read an agent's output without strain.
27. As a user, I want the cursor in an unfocused pane to draw hollow, so that only one cursor looks active.
28. As a user, I want a program in a pane to get focus-in and focus-out reports when its pane gains or loses focus, so that editors and TUIs react as they do to a window focus change.
29. As a user, I want only the panes of the tab on screen to resize their terminals, so that hidden tabs do not reflow while I work.
30. As a user, I want the sidebar to keep one row per session, so that I see the state of each agent.
31. As a user, I want a bracket in the sidebar to join the sessions that share a tab, so that I see which sessions are on screen together.
32. As a user, I want the sidebar rows of a tab in the reading order of its panes, so that the sidebar matches the screen.
33. As a user, I want clicking a sidebar row to show its tab and focus its pane, so that one click takes me to that session.
34. As a user, I want clicking a row of a pane hidden by a zoom to unzoom the tab, so that the session I chose is visible.
35. As a user, I want the agent ring keys to show the agent's tab and focus its pane, so that I can step through my agents across tabs.
36. As a user, I want the tab strip to show one entry per tab, so that the strip stays short.
37. As a user, I want a tab's entry in the tab strip to carry the most urgent state of its sessions, so that a blocked agent in a split still catches my eye.
38. As a user, I want Ctrl+Tab and Ctrl+Shift+Tab to cycle tabs, so that the keys move between screens and not between panes.
39. As a user, I want dragging a sidebar row to move its whole tab, so that reordering never breaks a split.
40. As a user, I want moving a session to another workspace to take it out of its pane into a new tab there, so that the split it leaves closes the gap.
41. As a user, I want to open a scratchpad, a task list or a diff in a pane beside a shell, so that I can take notes or read a diff while I work.
42. As a user, I want a scratchpad or task list that opens as its own tab to keep returning me to the previous session when it closes, so that this behavior does not change.
43. As a user, I want splits to work for native sessions, so that I get splits without changing my session backend.
44. As a user, I want splits to work for `herdr_core` sessions, so that persistent sessions can share a tab.
45. As a user, I want a new pane to open on the workspace's side, so that a split of a WSL checkout opens in that distro.
46. As a user with `herdr_core` sessions, I want my tabs and their panes back in place after a quit, a crash or a reboot, so that I do not rebuild my layout.
47. As a user, I want a pane whose terminal is gone at relaunch to be dropped and its sibling to take the space, so that a restored tab never shows a dead pane.
48. As a user, I want native sessions to be dropped from a restored tab, so that a restored tab holds only what survived.
49. As a user, I want a scratchpad pane to come back with its file, so that my notes stay beside my agent.
50. As a user, I want a restored tab with no surviving panes to be dropped, so that no empty tab appears.
51. As a user with two alacritree windows on one core, I want the edits of one window to survive the edits of the other, so that neither window wipes the other's layout.
52. As a user, I want a terminal that an agent labels with my workspace, and that no tab references, to arrive as a new tab without a change of focus, so that an agent never takes my keyboard.
53. As an IPC client, I want `create_session` to take an optional split of an existing session and a direction, so that an agent can open a pane beside itself.
54. As an IPC client, I want `list_sessions` to report each session's tab and pane rectangle, so that an agent can see the layout.
55. As a user, I want the name "foreign terminal" for a terminal that alacritree lists without having opened it, so that "pane" means only a region of a tab.
56. As a developer, I want every tab to hold one pane before splits ship, so that the tab model lands with no visible change.
57. As a developer, I want split behavior tested through the app's actions and IPC replies, so that a change to the tree's internals cannot make the tests lie.

## Implementation Decisions

### Vocabulary

- A session is one terminal, scratchpad, task list or diff. It keeps its agent state, foreground program, labels and sidebar row.
- A tab groups sessions, each in its own pane. A workspace remembers its active tab, and a tab remembers its focused pane.
- What the code and UI call a pane today becomes a foreign terminal. The rename covers the multiplexer integration crate, the app and the UI text. The zellij and herdr backend crates keep "pane" where they quote those tools.

### Layout tree

- Each tab holds an n-ary tree. A leaf is a pane that shows one session. A container is a row or a column of children, and each child has an `f32` weight.
- A container has at least two children. A container left with one child collapses into its parent.
- A split inside a container on the same axis inserts a sibling next to the target, and the new pane takes half the target's weight. A split on the other axis wraps the target in a new container.
- Closing a pane gives its weight to the sibling before it, or to the sibling after it when it was first, and then collapses as above.
- Equalize sets every weight in the tab to the same value.
- Zoom is a per-tab flag that names one pane. A split, a close of the zoomed pane, or a focus change that a row click or the agent ring makes clears it.
- Weights are fractions, so a window resize needs no redistribution.

### Sizes

- The minimum pane size is 10 columns by 3 rows. A divider drag or a keyboard resize stops at that limit. A split that would cross it is refused with a toast.
- The keyboard resize step is 2 cells, converted to a weight change on the nearest container on that axis.
- Only the panes of the tab on screen resize their terminals. A hidden tab keeps the last sizes until it shows again.

### Operations

- Split right, left, down and up. The default keys are Alt+Shift+Plus for right and Alt+Shift+Minus for down. Left and up are palette actions.
- Focus by direction, with Ctrl+Alt+Arrows. The candidate panes overlap the focused pane on the perpendicular axis. The nearest gap wins, then the most recent focus.
- Focus previous and next, in reading order, as palette actions.
- Resize, with Ctrl+Alt+Shift+Arrows and divider drag.
- Equalize and swap, as palette actions. Swap exchanges the focused pane with its neighbour in a direction, and the weights stay with the places.
- Zoom toggle, with Ctrl+Shift+Z.
- `CloseSession` closes the focused pane's session. The last pane closes the tab through the existing close landing and `return_to_previous_session`. Inside a tab, focus goes to the most recently focused pane.
- A new pane takes the focused session's shell profile and cwd when that cwd is inside the workspace. Otherwise it takes the workspace default from the existing shell decision.
- Scratchpad, task list and diff sessions can sit in panes. Opening one in a tab with several panes splits the focused pane. `returns_to` applies only when one opens as its own tab.
- A click on an unfocused pane focuses it and does not reach the program. There is no focus-follows-mouse.
- A pane focus change sends focus-in and focus-out reports, as a window focus change does. An unfocused pane draws its cursor hollow.

### Tab-level behavior

- `SelectNextTab` and `SelectPreviousTab` cycle tabs.
- A sidebar row click and the agent ring show the session's tab, focus its pane, and unzoom the tab when another pane is zoomed.
- Dragging a sidebar row to reorder moves its whole tab.
- `move_session` takes the session out of its pane, and it arrives in the target workspace as a single-pane tab.

### UI

- The tab strip shows one entry per tab. The entry's attention mark is the most urgent state among the tab's sessions.
- The sidebar shows one row per session. A bracket joins the rows of a tab that has more than one pane. Rows are in tab order, then in the reading order of the panes.
- A divider is 1 px in the sidebar's border color, with a 6 px hit area. On hover it becomes a 3 px accent line, and the cursor becomes `ResizeColumn` or `ResizeRow`.
- The focused pane gets a 1 px accent outline when its tab has more than one pane. Unfocused panes are not dimmed.
- No config option gates splits or their visuals.

### IPC and MCP

- `create_session` takes an optional `split` with `of`, a session id, and `direction`.
- `list_sessions` reports each session's tab id and pane rectangle.
- Each new operation is one `IpcRequest` variant. MCP tool names match its serde tags.
- Resize and zoom have no IPC operation.

### Persistence

- The side of a workspace decides where its new panes open and which core holds its trees. A program such as `wsl.exe` that runs inside a native session does not change the session's side.
- Each side has one core document named `alacritree`, which holds the trees of that side's workspaces.
- The tree in memory is the live truth, and the document is a mirror. alacritree writes a tab to the document when the tab holds at least one `herdr_core` session. A tab of native sessions only is not written.
- A leaf in the document names its session by the core terminal's address, or by the scratchpad's path.
- A write uses the document version for compare-and-swap. When the core refuses a write, alacritree reads the document again, applies this window's changed tabs over it by tab id, and writes again.
- At launch, alacritree rebuilds each tab in the document against the core's terminal listing:
  - A leaf with a live terminal is adopted into its pane.
  - A leaf whose terminal exited, was killed or is not found is pruned, and its container collapses.
  - A native leaf is pruned.
  - A scratchpad leaf reopens by path. Diff and task list leaves are pruned.
  - A tab with no leaves left is dropped.
- The document decides placement. Labels keep ownership and `alacritree.workspace`. A labelled terminal that no document references is adopted as a new single-pane tab at the end of its workspace, with no change of focus. alacritree writes no position label. If #430 ships a position label first, this work removes it.
- The focused pane of each tab is stored in the document. Zoom is not stored.

### Delivery

The work ships as a stack, in this order:

1. Rename pane to foreign terminal, in code and in UI text.
2. Introduce tabs, where each tab holds one pane. No visible change.
3. Splits: the tree, the operations, the keys, the dividers, the sidebar bracket and the tab strip, for native sessions, in memory.
4. IPC: the `split` field on `create_session`, and the tab and pane rectangle in `list_sessions`.
5. Persistence in the per-side `alacritree` document. This stage lands after the `herdr_core` session backend from #430.

## Testing Decisions

- A good test drives behavior that a user or an IPC client can see. It dispatches the same binding actions and IPC requests that a user or an agent sends, and asserts on the focused session, the tab list and its order, each session's tab and pane rectangle as `list_sessions` reports them, the sidebar row order and brackets, and toasts. It does not assert on the tree's internal shape.
- **Seam 1: the app.** Tests build the app with pending sessions and no PTY. They cover split and collapse, the new pane's cwd and profile, directional focus and its tie-breaks, the size limit and refused splits, keyboard resize, equalize, zoom and unzoom, swap, close landing, Ctrl+Tab, the agent ring, row reordering, `move_session`, and the IPC `split` field. Geometry edge cases go through the rectangles in `list_sessions`. Prior art: the app tests that build a test app and drive tab takeovers and close landing, and the IPC handler tests.
- For persistence, the same seam uses the scripted session backend fake from #430 with an in-memory document store. Those tests cover which tabs the document mirrors, the merge after a refused compare-and-swap, and each restore rule.
- **Seam 2: the `hc_e2e` suite** that #430 adds, against a throwaway core. It gains a round trip of the `alacritree` document through `doc.get` and `doc.set`, including a version conflict.
- The divider hover visuals and the cursor icon get manual verification.

## Out of Scope

- Moving a pane to another tab by drag and drop, or joining two tabs.
- An automatic split direction.
- A split button on pane edges.
- Dimming of unfocused panes, and any config option for splits.
- Pixel sizes and per-pane minimum or maximum sizes in the tree.
- Focus-follows-mouse.
- Resize and zoom over IPC.
- Persistence of native-only tabs. Native sessions do not survive a quit.
- herdr-app's `herdr-app` document. alacritree neither reads it nor writes it.
- A tree that spans sides. A tab's sessions open on its workspace's side.

## Further Notes

- The research behind this spec read the pinned sources of ghostty, kitty, wezterm, herdr, herdr-app and herdr-core.
  - ghostty, kitty, wezterm and herdr use binary trees with a ratio or a bias per split. herdr-app uses row and column containers with weights, which this spec follows.
  - All of them give a closed pane's space to its sibling. All of them pick a directional neighbour by perpendicular overlap, nearest gap and focus history.
  - herdr-core has no layout concept. It owns flat terminals with labels, and it stores named JSON documents with version compare-and-swap. herdr-app keeps its tree in such a document per machine, and replaces its local tree with the remote one on a conflict. This spec merges by tab instead, so that two alacritree windows do not wipe each other's edits.
  - A core sizes a terminal to its most recently active viewer that is not a watcher. When herdr-app shows the same terminal at another size, the two clients alternate, as they do without splits. This spec does not change that rule.
- The default keys avoid the current bindings: Alt+Left and Alt+Right for workspaces, Alt+Shift+Up and Alt+Shift+Down for the agent ring, Ctrl+Shift+O for adding a project, and the font size keys. The split keys follow Windows Terminal from memory, and nobody has checked them against its source.
- Terms: a session is one terminal, scratchpad, task list or diff with one sidebar row. A tab groups sessions in panes. A pane is the region of a tab that shows one session. A foreign terminal is a terminal that alacritree lists without having opened it.
