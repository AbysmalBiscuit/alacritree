# herdr event subscription

Issue: AbysmalBiscuit/alacritree#69. Targets herdr 0.9.1 (protocol 22).

## Goal

The sidebar follows herdr as it changes. Today alacritree spawns `herdr pane list` every `poll_interval_ms` (2 s by default), so a pane or focus change inside herdr reaches the sidebar 1 to 2 s late, and on WSL each poll is a `wsl.exe` spawn that gets slow under load. Replace the timed poll with herdr's event subscription on every side, native and WSL.

Arnaud does not use herdr, so there is no gate: subscription replaces polling outright, and `poll_interval_ms` is deleted (alacritree ignores unknown keys, so configs that set it keep loading).

## herdr facts this relies on

Verified against the v0.9.1 source.

- The API socket speaks newline-delimited JSON, `{"id","method","params"}` in, `{"id","result"}` or `{"id","error"}` out. Each connection carries one request.
- `events.subscribe {"subscriptions":[{"type":...}]}` answers `{"result":{"type":"subscription_started"}}`, then streams `{"event":<name>,"data":{...}}` lines until the server exits. Lifecycle kinds start live, with no replay. There is no keepalive and no sequence number.
- The server delivers at most one event per subscription per 100 ms tick. The event ring holds 512 events and drops the oldest silently.
- `pane.agent_status_changed` needs a `pane_id` per entry, and subscribe fails outright if any listed pane is unknown. Status changes emit nothing else.
- `pane.focused`, `tab.focused` and `workspace.focused` report the server's default target, which every TUI navigation and every socket focus request moves. That target is also what `pane list`'s `focused` field reports.
- `herdr remote-api-bridge` exists in 0.9.1 and not in 0.8.2. A herdr without it lists nothing in alacritree. It connects to the API socket herdr itself would use and pipes stdin to the socket and the socket to stdout. It exits on a connect failure (message on stderr, non-zero status) and when the server closes the stream. On Windows it also stops when its stdin reaches EOF, so stdin stays open for the bridge's whole life. `remote-api-bridge --check` prints `herdr-api-bridge-v1`. The command is undocumented.

## Design

### Transport

Each side runs `herdr remote-api-bridge` as a child process: directly on the native side, through `Side::command` (`wsl.exe --exec sh -lc`) on a WSL side. herdr resolves its own socket path, so alacritree never re-implements herdr's `XDG_CONFIG_HOME`, `%APPDATA%` and session rules, and a bridge reaches the same server the CLI calls do. A reader thread per bridge splits stdout into lines, decodes them, and sends them to the UI thread over a channel, waking it with `request_repaint`.

### Two streams per side

- **Lifecycle stream.** Subscribes to a fixed set of kinds: `pane.created`, `pane.closed`, `pane.updated`, `pane.focused`, `pane.moved`, `pane.exited`, `pane.agent_detected`, `tab.closed`, `workspace.closed`. TUI navigation emits `pane.focused` alongside the tab and workspace focus events, so those two add nothing. It is never rebuilt while herdr is up.
- **Status stream.** Subscribes to `pane.agent_status_changed` for every listed pane that has an agent. Each entry costs herdr a pane lookup per delivery tick, so shells are left out, and one that gains an agent announces itself through `pane.agent_detected`. It is rebuilt whenever that set changes, and its ack starts one listing, since herdr does not stream a status a pane already had. Rebuilding it never touches the lifecycle stream. A refused subscribe is remembered against the pane ids it named and not retried until a listing names a different set.

### Applying events

- `pane.agent_status_changed` patches `Pane.status` for its `pane_id` in the endpoint cache.
- `pane.focused` sets `Pane.focused` on its `pane_id` and clears it on every other pane of that side.
- `pane.updated` carries the whole pane in `pane list`'s shape and replaces the row it names. An update for a pane the side does not show is ignored, since a shell's title churns with every prompt.
- A patch that changes a rendered field bumps the cache generation, which redraws the sidebar, and advances the side's sample time. `HerdrViewSync` follows only a sample newer than its own last focus move, so without that a focus patch would wait for an unrelated listing.
- herdr delivers focus events in order, so after alacritree moves herdr's focus to a pane itself, a focus event naming any other pane that arrives before herdr's echo of the move predates it. Such an event is not applied or queued; it starts a listing instead. The distrust ends at the echo, or at a listing sampled after the move, which covers the move herdr echoes nothing for because the pane was already focused.
- alacritree moves herdr's focus with the socket's `pane.focus`, sent down a one-shot bridge, which names the pane itself. The CLI cannot name a pane with no agent: `agent focus` refuses it, and `tab focus` lands on whichever pane of the tab herdr last focused, so a switch between shells sharing a tab would echo the old pane and the sidebar would follow it back.
- Every other lifecycle event marks the side's listing as due, and the cache runs one `pane list` for the full rows. Events arriving while that listing is in flight coalesce into one follow-up listing, so a burst costs two listings at most.
- Patches that arrive while a listing is in flight are applied at once and also queued, then applied again on top of the listing when it lands, following herdr's documented subscribe, ack, snapshot, apply order. A listing that fails drops the queue.

### Connection lifecycle

| Situation | Behaviour |
|---|---|
| herdr not running | The bridge exits with the connect error. The side is down and retried after 0.5 s, 1 s, 2 s, then every 5 s. A user action that targets the side (opening a herdr row, a palette herdr command) retries at once. |
| Ack arrives | The side is up. One `pane list` seeds the rows, then the status stream opens for the pane ids it returned. |
| herdr exits or restarts | The bridge sees EOF and exits. The cached session name is forgotten and the side reconnects at once, then backs off as above. Rows survive the existing grace window, and the reconnect's listing replaces them entirely. |
| No herdr binary on a side | The spawn fails or the login shell reports it missing. The side is abandoned for the process lifetime, as `Reach::abandoned` does today. Stopped WSL distros are never endpoints, so a retry never boots one. |
| Incompatible or unexpected reply | An error line or an unparseable ack is logged once through `Reach`'s dedupe and retried on the 5 s cadence. |
| herdr hangs without exiting | Undetectable from the stream. An attach or focus gesture that times out restarts that side's bridges. |

The timed poll, its interval, and `poll_interval_ms` are removed. `pane list` remains as the fetch the cache runs after an ack or a structural event. There is no periodic resync: the only source of drift is a ring overrun, which needs hundreds of events inside one delivery tick, and every reconnect relists from scratch.

### Code layout

- `herdr/events.rs` (new): the bridge child, the subscribe request lines, the reader thread, and decoding a line into a typed event. Pure parsing is unit-tested against captured 0.9.1 lines.
- `herdr/cli.rs`: stays the only file that builds herdr argv, including the bridge's.
- `herdr/poll.rs`: `EndpointCache` owns the streams, the backoff, the listing trigger, and applying events. The interval-driven `poll_due` goes away.
- `herdr/host.rs`, `multiplexer/`: `poll` loses its interval argument. A gesture timeout asks the cache to restart its bridges.
- `config.rs`: `poll_interval_ms` and `HerdrConfig::poll_interval` are deleted, and the schema is regenerated.

## Testing

- Decoding: each subscribed kind, the ack, an error line, unknown kinds and fields.
- Cache: a status event patches the pane and bumps the generation; a focus event moves focus within the side; a structural event marks the listing due; events during an in-flight listing are applied after it lands; a pane-set change rebuilds the status subscription and a steady set does not.
- Lifecycle: a bridge exit marks the side down and schedules the backoff; an ack triggers exactly one listing; a side with no binary is abandoned.
- Streams are exercised through a fake bridge (a channel of lines), not a real herdr. The opt-in `e2e` task covers a real server.
