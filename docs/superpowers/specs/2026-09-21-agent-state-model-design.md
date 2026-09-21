# Agent state model: done, unknown, and a ping of its own

Issue: AbysmalBiscuit/alacritree#66 (child of #19).

## Goal

Every agent row in the sidebar shows one mark. Each mark means exactly one thing, so a user can tell what a session is doing from the glyph alone. The model grows from idle, working and blocked to six shown states, and native sessions gain the `done` and `unknown` states that herdr panes already report.

This ships ungated. It expands the state model rather than adding an optional view.

## States

Two axes stay separate in the model and fold into one mark at paint time.

- **Live state**, recomputed from what the session shows right now: `unknown`, `idle`, `working`, `blocked`.
- **Latches**, set by an event and cleared by the user looking at the session:
  - `done`: the agent finished a turn while nobody was watching.
  - `pinged`: the terminal rang (BEL).

A row with no agent has no live state and can only show `pinged`.

### What the row shows

One mark, the highest-ranked `ShownState` that applies:

`blocked > done > pinged > working > idle > unknown`

A latch that a louder state hides stays set, and shows once that state clears, unless the user looks at the session first.

| shown state | glyph | colour | hover |
|---|---|---|---|
| blocked | `!` | Blocked tone (palette red) | `<agent> is waiting for you` |
| done | `✓` | Done tone (palette cyan) | `<agent> is done` |
| pinged | `•` (the painted attention dot) | attention colour (palette yellow) | `needs attention` |
| working | braille loader, unchanged | accent (palette blue), unchanged | `<agent> is working` |
| idle | `◇` | Idle tone (palette green) | `<agent> is running`, unchanged |
| unknown | `?` | Unclear tone (muted text) | `<agent>, state unknown` |
| no agent | row's session icon, unchanged | unchanged | none |

`•` means pinged and nothing else. Blocked moves off `●` because at sidebar size `●` and `•` read as the same mark, and `×` is already the row's close button.

## Where each state comes from

### Native sessions

- **working**: a Braille spinner title, as today.
- **idle**: an agent is present (process probe or decorative title glyph) and nothing else applies, as today.
- **unknown**: the title carries a decorative agent glyph, the probe recognized no agent, and the probe could not ask. It cannot ask when there is no shell pid yet, or when a WSL session's helper gives no answer. `process_probe::Signals` gains a field saying the probe answered. An answered probe that found no agent keeps today's reading, idle.
- **blocked**: nothing native produces it yet. Detection arrives with the herdr integration and the manifest engine, as #66 already scopes.
- **done**: the title goes from a spinner to a plain title while the session is not visible to the user. Today that transition sets the ping (`apply_term_event`, `session.rs`). It sets `done` instead.
- **pinged**: BEL, as today. The title transition above no longer pings.

`DrainOutcome.attention: bool` splits into two triggers, finished and rang. Both go through the existing grace debounce, so an orchestrator that drops its spinner between tasks still produces no `done`. Each trigger latches its own flag when the grace window passes.

Notifications fire when either latch sets, if it wasn't already set, so the toasts people get today are unchanged. `blocked` does not notify.

`done` clears when the user views the session, the same way `needs_attention` clears, and also when the session goes back to working. At that point it no longer describes anything.

### Pane-backed rows (herdr, scripted)

The harness's own status still outranks alacritree's reading of the title. `PaneStatus` already has all five live values. `PaneStatus::Done` maps to shown `done` and `PaneStatus::Unknown` to shown `unknown`. Today `LiveState::from_pane` folds done into idle. The session's own `pinged` latch still applies on top by rank.

### Debounce

`poll_attention_debounce` reads the session's live state (pane status included) instead of the raw title. `working` cancels a pending trigger. `blocked`, `idle` and `unknown` do not. A herdr pane that reports working now cancels a premature ping, which the title check missed.

## Rendering

- `widgets.rs`: `SessionMark::Attention` and `SessionMark::Agent(LiveState)` become one `SessionMark::State(ShownState)`. `session_status_mark` computes the shown state from the rank above instead of putting attention first. `agent_mark` gains arms for done and unknown. The sidebar row and the palette row both call `session_status_mark`, so they stay in agreement.
- Glyph constants in `config.rs`: `DEFAULT_BLOCKED_ICON` becomes `!`, and `DEFAULT_DONE_ICON = "✓"` and `DEFAULT_UNKNOWN_ICON = "?"` are added. The baked-glyph coverage test covers them.

### Harness marks and `use_integration_icons`

`[integrations.herdr] use_integration_icons`, default `false`.

- `false`: herdr rows draw the table above, the same as native rows.
- `true`: herdr rows draw herdr's own marks, following herdr's `status_indicators` (dots or symbols), as they do today. With herdr's default dots set, working, blocked and done all draw `●`. The field's doc comment says so.

The scripted multiplexer has no vocabulary of its own (it draws `*`), so it always uses the table above.

## Workspace and project rows

- The attention filter (`workspace_needs_attention`) matches a workspace when any of its sessions is blocked, done or pinged.
- `workspace_activity` picks its mark by rank. A blocked, done or pinged session anywhere in the workspace wins. Otherwise the active session's state wins, as today.

## IPC and MCP

`activity_json`'s `state` field gains `"done"` and `"unknown"`. `needs_attention` stays as the pinged latch. Consumers that only knew idle, working and blocked get two new string values and no removed ones.

## Testing

Tests go through the real entry points wherever the behaviour lives there.

- `shown_state`: a table test over every combination of live state (none included), `done` and `pinged`, asserting the rank.
- `apply_term_event`: a spinner-to-plain title sets the finished trigger and not the rang trigger. BEL sets only rang.
- A background native session driven through `process_session_events`:
  - spinner, then plain title, past the grace window: the row shows done and the notification fires once.
  - BEL on a working session inside the grace window is cancelled by the debounce, so no ping latches.
  - BEL on a session whose herdr pane reports blocked latches pinged. The row shows blocked, then pinged once herdr reports idle.
  - viewing the session clears both latches.
- Debounce: a pending trigger on a herdr pane reporting working is cancelled, one reporting blocked is not.
- Native unknown: a decorative-glyph title with a probe that could not ask shows unknown; the same title with an answered probe shows idle.
- `session_status_mark` for a herdr row with `use_integration_icons` on and off.
- Glyph uniqueness: no two shown states share a glyph, and none use the attention dot except pinged.

## Out of scope

- Native `blocked` detection.
- Notifications for `blocked`.
- A separate `use_integration_icons` for zellij or other harnesses. The key lives under herdr until another harness brings a vocabulary of its own.
