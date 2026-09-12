# Following herdr's focus from any session

An adversarial review of this design against the source sits beside it in `2026-09-09-herdr-follow-focus-review.md`, with a `file:line` citation for every claim. This document is the design as it stands after that review.

## The problem

Creating or focusing a pane inside herdr moves alacritree to the matching session only while the already-active alacritree session is itself herdr-backed. From a plain native session, nothing happens.

`sync_herdr_view_focus` in `app.rs` derives both the selection and the endpoint snapshot from the active session's `herdr_key`. A native session has none, so both come out `None` and `HerdrViewSync::next` returns before it ever looks at which pane herdr reports as focused. The endpoint is polling the whole time and the answer sits in the cache unread.

This is deliberate rather than an oversight. Two tests pin it: the `None`-active assertion inside `herdr_shared_view_follows_new_tabs_and_refocuses_on_return`, and `herdr_shared_view_refocuses_after_an_ordinary_session`.

Following is also a heavier gesture than a selection change. From a native session it can spawn a new attach client, switch workspace, move the sidebar cursor and take terminal focus. And it fires precisely when the user is typing, because `sync_herdr_view_focus` only reads the focus snapshot while the terminal has focus and no modal or palette is open. So it needs a gate, which `AGENTS.local.md` requires of any new UX feature anyway.

## Decisions

The gate is tri-state rather than the bool `deferred.local.md` proposed. A bool defaulting false would turn off following that already ships and is in daily use; a bool defaulting true would add new behavior to an unmodified config. A tri-state whose default is today's behavior does neither.

Following in `always` mode acts on any reachable side. A herdr in a distro the user has not attached to today can still pull them into a new session, which is the accepted cost of the simplest and most predictable rule.

Following from a native session waits for a gap in direct input and gives up if that gap never comes. Following moves the keyboard, and in `always` mode a pane a script created can arrive while the user is mid-command. Delivering such a follow late is worse than dropping it.

`ListSessions` grows enough vocabulary to say what runs in a session and where that session lives. The immediate reason is that the end-to-end test cannot otherwise assert which pane alacritree landed on. The lasting reason is that alacritree intends to support other multiplexers, and the difference between a native session and a multiplexer-backed one, plus which multiplexer, is what every consumer of that surface will want to know first.

The end-to-end suite pilots the real binary rather than driving `AlacritreeApp` in process.

## 1. The config option

`[integrations.herdr] follow_focus`, one of `"off"`, `"herdr"` or `"always"`, default `"herdr"`.

- `"off"` suppresses the `Follow` arm entirely.
- `"herdr"` follows only while the active session is herdr-backed. This is what ships today, so an unmodified config sees no change.
- `"always"` additionally follows from a native session.

A `FollowFocus` enum on `HerdrConfig`, and a `parse_follow_focus` mirroring `parse_attach_mode`, which warns on an unknown value and falls back to the default.

On `RawHerdr` the field is `follow_focus: String` under `#[serde(default)]` rather than `Option<String>`, carrying `"herdr"` from the type's `Default`. That is what `AGENTS.md` asks of every key whose omission resolves to a fixed value: runtime resolution and schema generation share one source, and the default reaches the published schema. It diverges from `attach` sitting beside it, which is `Option<String>` and publishes no default; the stacked config-defaults PR owns converting that one, and this field is written the way that PR will leave it rather than the way it finds it. The field also takes `#[schemars(extend("enum" = ["off", "herdr", "always"]))]`, as `attach` already does.

Regenerate the schema with `ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema`. `config_schema` is the test that exists on this branch and has to pass; `schema_defaults` and its allowlist arrive with the config-defaults stack and are not here.

The `Focus` arm keeps running whatever the setting says. That arm is alacritree telling herdr where to point when the user picks a row, and a shared view draws the wrong pane without it. The setting governs whether herdr may move alacritree, not whether alacritree may move herdr.

## 2. Following from a native session

`HerdrViewSync` keeps one entry point, so the precedence between the two ways a follow can arise lives in one function rather than in a call ordering inside `app.rs`. It gains a private focus trail: per side, the last focused terminal id and the sample time that established it. `Side` and `HerdrKey` both derive `Hash`.

The caller stops deriving one snapshot from the active key and passes every endpoint cache:

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

The arguments arrive as a struct because there are now eight of them, which is one past what clippy accepts, and because named fields at the call site read better than a row of bare booleans. `now` is passed rather than read inside so the hold-off can be tested without sleeping.

`active` carries the unfiltered key. Today `next` drops a direct-attach session before anything else, which is right for the shared-view path and wrong for the trail: a session that attaches directly still occupies a pane, and a trail that cannot see its key would follow the user to the pane they are already on. The direct-attach filter moves inside the shared-view path.

`attentive` is what `app.rs` computes today to decide whether to pass a snapshot at all. It becomes a flag rather than a pre-filter because the trail must not record a change that happened while the user was elsewhere. Recording only while attentive is what preserves today's catch-up-on-return behavior, where a focus change made in herdr while alacritree was in the background is acted on when the user comes back.

Passing the caches rather than a built slice of snapshots is allocation-free: `caches()` already returns a slice, and only a changed id clones a `String`.

`next` also has to be restructured before any of this is reachable. It returns at `active?` today, before it reads a snapshot at all, so a native session never gets past the first third of the function. The trail path runs where that early return is.

### Precedence

1. The shared-view path runs first and is unchanged, watermark and all. Under `"off"` its `Follow` result is suppressed and its `Focus` result is not.
2. Only when that path yields nothing, the mode is `"always"`, and `attentive` is true, the trail speaks. It compares each side's currently focused terminal id against what it recorded, and takes the first side that changed, walking the caches in their own order. Two sides changing between one frame and the next is rare enough that a deliberate tiebreak would be inventing a rule nobody can observe; the other side's change is not lost, because it is still a change on the next frame.
3. That change does not become a `Follow` straight away. It becomes the pending follow described below, and `next` emits it on a later frame or drops it.
4. A side seen for the first time records without proposing anything, so starting alacritree never yanks the user somewhere.
5. A change whose target is already the active session's own pane records without proposing.

### The trail's watermark

Three paths move herdr's focus without the trail proposing anything: the `Focus` arm, the attach gesture, whose `herdr_attach_gesture` focuses the pane before attaching, and the shared-view `Follow`. Without a defense, opening a herdr row from the palette and switching straight back to a native session inside one poll interval pulls the user back to the row they just left.

So a trail entry is a terminal id and a stamp, and the stamp is a watermark on samples the way `follow_after` already is for the shared-view path. A sample whose `sampled_at` is at or before the entry's stamp cannot form an edge. This is what stops a listing that was already in flight when the focus moved from running the edge backwards, which is the same race `next` already rejects at `sampled_at <= self.follow_after`.

Everything that moves herdr's focus stamps the trail for that side with the terminal it targeted and the moment it did so: `attached`, the `Focus` arm's completion in `settled`, and the attach gesture. `attached` takes only a session id today, so it gains the key or `app.rs` stamps alongside it.

Proposing does not stamp. Delivering does, and so does giving up. `follow_herdr_view` returns silently when the herdr session name has not been read yet, which is exactly the state right after a server start, and when the row resolves to no workspace. Were the trail stamped at proposal time, that first follow would be recorded as handled and lost for good, where the shared-view path survives the same gap by proposing again on the next newer sample. Stamping on delivery gives the trail the same self-healing. Expiry stamps too, because deciding not to go is a decision, and without it an undeliverable target would be re-proposed forever.

So the trail is edge-triggered with a per-side watermark, not edge-triggered instead of watermarked.

### What is not an edge

The listing does not always name a focused pane. Under `show_panes = false` an unattached side lists only panes herdr found an agent in, so a new shell tab is invisible and the focused id is `None` whenever herdr's focus sits on an unlisted pane. A failed poll empties the list entirely.

A side reporting no focused pane leaves its trail entry untouched and never forms an edge, and so does a side that is not answering. An id appearing after `None` is first sight, not a change. A side whose cache disappears is forgotten, because `Endpoints::adopt_running` drops a WSL cache when its distro stops and builds a fresh one when it starts, and a trail keyed only by `Side` would otherwise compare across that gap.

### Holding off while the user is working

Following is not a passive highlight. It calls `activate_session_by_id` and then `focus_terminal()`, so the keyboard moves with it. And `attentive` means it fires only while the window is focused and the terminal has pane focus, which is to say exactly while the user is at the keyboard. A pane created by a script or an agent hook can therefore land the tail of a half-typed command in a shell the user never chose.

So a trail edge does not act immediately. It becomes a pending follow carrying the target key, the side it came from, and the moment it was proposed.

It waits for a gap in direct input. The clock is a new field, not the existing `last_input`, which advances on any non-empty event list including pointer motion and window focus and which the liveness probe already reads as its grace period. The new one is fed by key presses, text, scroll, pointer buttons, copy, cut, paste and zoom; bare pointer motion is left out, or hovering the mouse over the window would hold a follow off forever. `WindowFocused` restarts the gap, so a follow waiting when the user alt-tabs back lands shortly after they return rather than the instant they do.

It expires. A change the user typed straight through for long enough is stale, and moving them then is worse than not moving them at all, so the pending follow is dropped without acting rather than delivered late.

The newest change wins. A second edge while one is pending replaces the target and restarts its clock, because the pane herdr is on now is the only one worth going to.

Both clocks count attentive time only. Counting wall-clock time would expire a follow while the user was in another window, which is precisely the catch-up-on-return case the trail is built to preserve.

The pending follow is dropped when the active session changes, since it was proposed against a situation that no longer holds, and when the session holding its target closes, or a follow would respawn an attach client for a row the user has just closed. Its target being the active session is re-checked at delivery, not only when the edge formed. While `busy`, the pending neither advances nor expires: `next` returns before the trail path in that state, and an attach on Windows can hold it for seconds.

The gap is a constant and so is the expiry. The feature's knob is `follow_focus`, and timing values nobody has complained about do not need a second one. A gap of around 750 ms clears the pause between keystrokes, and an expiry of around 10 s matches `PROBE_GRACE`, which is the codebase's one existing answer to "the user is active". Both want a check against a real run.

Nothing here touches the watermarked shared-view path, which ships today without a hold-off and is in daily use. Extending it there later is a small change and a separate decision.

The two tests that pin today's behavior get parameterized by mode rather than deleted. Their assertions hold unchanged under the default `"herdr"`; their calls change, because the signature does.

## 3. What ListSessions reports

`session_json` gains three nullable fields, all from accessors the sidebar and command palette already use.

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

`agent` is `SessionActivity` rendered as JSON. `null` is `Shell`; the object is `Agent { name, live }`, where `name` may itself be null for an agent inferred from a decorative title prefix or vouched for by herdr without a process match, and `state` is `LiveState::label`, one of `idle`, `working` or `blocked`. Native probing and herdr's status already collapse into this one vocabulary through `LiveState::from_herdr`, so one field covers both kinds of session.

`busy` says whether anything holds the foreground beyond the shell itself. It is null for a multiplexer-backed session, because the attach client is itself that foreground job and the probe would answer `true` forever. That is the same reason herdr shell rows carry no state today.

`multiplexer` is null for a session owning its own PTY. `name` distinguishes herdr from whatever is supported next. `side` is `native` or `wsl:<distro>`, which is part of an agent's identity because two herdr servers on one machine cannot see each other. `session` is the herdr session name from `EndpointCache::session_name`, null until the first `session list` reply lands.

`side`, `session` and `terminal_id` are stored on the session and survive a failed poll. `pane_id` and `tab_id` come from `find_herdr_agent`, which reads the live listing, so they go null when a poll fails. Reading them instead through `attachment_pane`, which retains an attached row's pane with `current = false`, would keep the last known values across a failure; the design takes the live lookup, so a null means the cache genuinely does not know right now.

`session_json` is a free function over `&Session` today, and the new fields reach past it. `agent` folds the session's own activity together with herdr's status through `herdr_backed_activity`, and that status comes from the caches. `multiplexer` needs `find_herdr_agent` and `herdr_session_name`, both `AlacritreeApp` methods. So `session_json` becomes a method, or takes the caches as a second argument.

This is additive to a reply, not a new `IpcRequest` variant, so no request enum, MCP tool surface or config schema changes. `cli/render.rs` renders the new fields for the human-readable path.

## 4. The end-to-end harness

`alacritree/tests/herdr_e2e.rs`. `CARGO_BIN_EXE_alacritree` is only set for integration test targets, and piloting the real binary needs no in-crate access, so it belongs beside `cli_isolation.rs`, which already drives the executable this way.

The harness runs a throwaway herdr server and a real alacritree window against it.

**One environment for all three parties.** The throwaway server, every herdr command the harness runs, and the alacritree child all run under the same redirected `APPDATA`, `LOCALAPPDATA`, `XDG_CONFIG_HOME` and `HOME`, pointing at one temp directory. `run_isolated` sets all but `XDG_CONFIG_HOME` today, and herdr reads that one first.

Redirecting the environment for only the alacritree child, as an earlier draft of this design did, breaks twice in opposite directions. herdr's config directory on Windows is `%APPDATA%\herdr`, so the child's herdr children would look for `sessions/<name>/herdr.sock` under the temp directory and find nothing there, because the server was started under the real one. And a shared-view attach runs `herdr session attach <name>` where the name comes from `running_session_name`, which takes the first running session herdr lists and ignores `HERDR_SESSION` entirely; with the developer's default server up and visible, the child would attach its rows to it. Putting herdr's session directory in the temp directory for all three parties makes the developer's sessions invisible to `session list` and puts the harness server where the child looks.

The six inherited herdr variables still get cleared: `HERDR_ENV`, `HERDR_PANE_ID`, `HERDR_TAB_ID`, `HERDR_WORKSPACE_ID`, `HERDR_SOCKET_PATH` and `HERDR_CLIENT_SOCKET_PATH`. `HERDR_SOCKET_PATH` outranks `HERDR_SESSION`, and herdr sets it inside every managed pane, so a harness run from inside one would otherwise drive the pane it is running in. `HERDR_SESSION` stays set, so the session has a name for `server stop` and `session delete` to target.

**Two seatbelts.** Before any mutating herdr command, the harness runs `session list --json` in the child's environment and asserts it shows exactly one running session, named after this test, with its session directory under the temp directory. Comparing path components rather than a `sessions/<name>` substring, because Windows spells it with a backslash. Before any alacritree command, `ALACRITREE_SOCKET` is pinned to the socket named after the child's own pid: without it, a client that inherited no socket variable finds an instance by listing the pipe directory, and the developer's live GUI is in that directory. Both seatbelts exist because the isolation failed twice by accident during the probing that established this recipe.

**The test config sets `show_panes = true`.** A tab created on the test server runs a plain shell, and an unattached side asks `agent list`, which does not list it. Without this the child never sees the pane the test creates.

**Foreground.** Following only fires while the window is the OS-focused one, and nothing in the IPC reply carries window focus. The test asks Win32 `GetForegroundWindow` and `GetWindowThreadProcessId` and compares the answer against the child's pid. `windows-sys` is already a dependency and an integration test may use the crate's dependencies. A window that never took foreground fails with that message rather than passing quietly.

**Direct input.** `send-text` writes bytes to the PTY and `run-action` dispatches a binding action; neither produces an egui event, so neither moves the hold-off clock. The one test that exercises the debounce injects real input with Win32 `SendInput` into the foreground window, after the foreground check above has established that the window is the child's.

**Teardown, in order.** `server stop` first, so every attach client exits on its own and herdr runs its own shutdown. Then kill and wait the GUI child; `Quit` opens a dialog rather than exiting, so there is no clean remote shutdown, and its conpty children die with the pseudoconsole handle. Then kill the server child if `stop` timed out, since `herdr server` runs in the foreground and the harness holds its `Child`, and `stop` waits up to 15 s. Then `session delete`, which refuses a running session and so has to come last. `Child::kill` is `TerminateProcess` on Windows and skips any `Drop`, which is why `server stop` leads rather than follows.

**WSL sides.** The endpoint poll enumerates distros as well as the native side, and this machine runs a herdr in WSL too. The redirected environment does not reach inside a distro, because `wsl.exe` forwards nothing by default, so a WSL herdr would answer for the user's real session. The mitigation is `WSLENV=HERDR_SESSION`: the WSL side then looks for a session that does not exist there and gets `server_not_running`, which alacritree reads as a server error rather than an absent binary, so the side keeps retrying on the 30 s recovery cadence instead of being abandoned. Harmless, and no daemon is spawned, because alacritree only runs listings on a side until a pane is listed there and listings fail cleanly. `Side::command` runs the WSL invocation through `sh -lc`, which sources the login profile, so a profile exporting `HERDR_SOCKET_PATH` would outrank the forwarded name; the assertions name the pane they expect, so that surfaces as a loud failure rather than a silent pass.

**Piloting** goes through the binary's own CLI: `session list --json`, `run-action`, `send-text`, `read-screen`. Nothing reimplements the wire protocol, and the CLI path gets exercised too.

**A missing herdr binary** panics with a clear message rather than passing quietly. These tests are opt-in, so a hard failure is the honest outcome.

**Running them** is a devkit task of its own rather than a flag on `devrun task test`, so the ordinary suite cannot pick them up by accident. It goes in `devkit.local.toml`, which lives in the main checkout rather than this worktree and rides the `docs/specs-and-plans` branch:

```toml
[tasks.e2e]
description = "Run the opt-in herdr end-to-end suite against a throwaway server"
run = ["cargo", "nextest", "run", "-p", "alacritree", "--locked",
       "--test", "herdr_e2e", "--run-ignored", "ignored-only", "--test-threads", "1"]
```

**The costs, stated plainly.** The spawned window must hold foreground for the whole test, and anything that steals focus breaks it. The `SendInput` test types into whatever window has foreground, which is why it runs only after the foreground assertion. A GUI window appears while these run. Under nextest each test is its own process, so the environment manipulation needs no global mutex; under plain `cargo test` the `#[ignore]` keeps them out of Arnaud's CI entirely, which compiles the target and runs nothing.

## 5. The reselect bug

Reselecting a shared-view session after herdr moved it onto a new pane is supposed to put it back and does not. Detaching and reattaching is the only way out.

The recovery path reads correctly in the source. Selecting a session whose id differs from `HerdrViewSync::visible` clears `focused`, `needs_view_focus` then answers true, and `sync_herdr_view_focus` spawns `herdr::focus_pane` for that row's own pane.

The cause is in herdr's source, and the fix is upstream and installed.

herdr's headless server decides per request method whether a public API call may move what an attached client is drawing. At `b99002ac`, the revision this work started against, `Method::TabFocus` is in both gates and `Method::AgentFocus` is in neither. `985d3442`, under the subject "project public agent focus to attached clients", adds `AgentFocus` to both and projects the resolved focus to attached clients after a successful call. The pinned checkout now serves `120c6820`, which carries it. Both revisions call themselves 0.9.0, so only the commit distinguishes them.

What gets projected is a tab surface, a workspace index and a tab index, the same shape `TabFocus` already projected. Within a tab that holds a split, that puts the attached client on the right tab and says nothing about which pane it lands on.

That maps onto `focus_args` exactly. alacritree sends `tab focus <tab_id>` for a pane herdr found no agent in, and `agent focus <pane_id>` for every other pane. So before `985d3442` a stuck row holding an agent cannot be recovered, because `agent focus` moves the server and never reaches the client, while a stuck row running a plain shell recovers, because `tab focus` projects.

That was a testable prediction and the cheapest confirmation available: the bug should reproduce on an agent row and not on a shell row, on the same server, in the same session. The server now running carries the fix, so the discriminating case cannot be observed without putting the old binary back. What remains worth watching is the plain question of whether reselect recovers at all. If it does, updating herdr was the whole fix and alacritree needs no change: no attach-client respawn, no companion CLI bridge extension.

This supersedes the handoff note's reading, which was that stock Windows full clients retain client-local selection and that global CLI focus never reaches them. The runtime proof behind that note ran against a server with no client attached, so it could not observe projection either way.

The harness in section 4 is the only automated way to observe this class of bug, because a piloted alacritree in `attach = "session"` mode spawns real attach clients against the throwaway server. A headless probe cannot: it has no client to be stuck.

## Testing

End to end first, so the failing test reproduces the real behavior rather than the one line being changed.

1. `follow_focus = "always"`, a native session active, `tab create --focus` on the test server. alacritree moves to a row for the new pane, asserted through `session list --json` on `current_workspace`, `is_active_tab` and the new `multiplexer.terminal_id`. `is_active_tab` is per workspace, so the workspace has to be checked with it. The `--focus` matters: `tab create` alone does not move the server's focus, and without an edge the test would fail for the wrong reason. Fails today.
2. The same scenario at the default `"herdr"`. alacritree must not move. Passes today and locks the default against regression.
3. The same scenario at `"always"` with `SendInput` driving continuous input into the child's foreground window. alacritree holds off while the input keeps arriving, and moves once it stops. This is the hold-off end to end, and the one test that would catch a debounce wired to the wrong clock.
4. Unit tests on `next` for the per-side edge, the record-without-following first snapshot, a change on one side while another is idle, a change whose target is already the active pane, a side reporting no focused pane, and a sample older than the trail's stamp.
5. Unit tests on the pending follow: held while input keeps arriving, delivered after the gap, dropped once expired, retargeted by a second edge which also restarts its clock, dropped when the active session changes, dropped when its target's session closes, and frozen while busy.
6. Unit tests on stamping: a follow that cannot be delivered leaves the trail unstamped and is proposed again on the next newer sample; an expired one stamps and is not.
7. A unit test on `next` that `"off"` still returns `Focus` for a shared view that owes herdr a focus call. This is what keeps the `Focus` arm ungated.
8. Unit tests on `parse_follow_focus` for each accepted word, an unknown word, and omission.
9. The two existing shared-view tests, parameterized by mode, still asserting today's behavior under `"herdr"`, including that no hold-off delays them.

The ungated `Focus` arm is checked by unit test rather than end to end because opening a herdr row is a sidebar or palette gesture and alacritree exposes no herdr surface over IPC, so a piloted instance cannot select one. The first three tests need no such gesture: following is itself what creates the row. Building that surface is the deferred `herdr sn` item, several times the size of this work, and it is the only thing standing between this suite and covering the attach gesture too.

`follow_herdr_view` returns early unless `herdr_row_workspace` resolves, which applies when no session already holds the key, so the harness has to create panes somewhere alacritree maps to a workspace. With `show_unmatched = true` a Home row is enough.

## Limitations

The harness exercises the real window, the real endpoint poll, real attach clients and the real CLI. It does not assert anything about paint output.

The assertions name the pane they expect, so a WSL side that answers despite the forwarded session name surfaces as a loud failure rather than a silent pass.

herdr checks a protocol version before a CLI request and refuses with the code `protocol_mismatch` when the running server was built against a different one, which is the state after updating herdr without restarting its server. `send_request` runs that check and `send_request_unchecked` skips it, the latter being how `server live-handoff` reaches a server it may not match, since a handoff is itself the way out of a mismatch. alacritree reads any parseable code as a server error rather than an absent binary, so the side keeps retrying on the recovery cadence instead of being abandoned, and recovers when the server restarts. Worth knowing, because during that window every listing fails and nothing in the sidebar says why. The protocol version is unchanged between `b99002ac` and `120c6820`, so this particular update does not open that window.

`herdr server live-handoff` moves live panes to a new local server, which is the upgrade path that does not take the user's running agents down with it. Restarting the server does.

## Unresolved questions

1. The quiet gap and the expiry, proposed at 750 ms and 10 s, want a check against a real run rather than a derivation. The poll interval already puts up to two seconds of age on a change before it is ever seen, which is the floor both sit on.
2. Whether reselect recovers against the server now running. The agent-row-versus-shell-row discriminator section 5 predicts needs a pre-`985d3442` binary to observe, so the practical check is whether a session dragged onto a new pane comes back.
