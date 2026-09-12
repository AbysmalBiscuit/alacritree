# Review: following herdr's focus from any session

Reviewed against `feat/herdr-palette` at `bbc6f346`. herdr citations were first taken from a 0.8.2-preview checkout and have since been reverified line by line against the pinned checkout `docm` serves, `120c6820`, which is the revision the installed binary and the running server were built from. `Cargo.toml` still says 0.9.0 there, so the version string does not distinguish the two.

Counts: 4 critical, 8 major, 11 minor. Critical and major findings change the design or the test plan; minor ones are wording or a one-line addition.

Paths are relative to `alacritree/src/` unless they start with `herdr/`, which is the herdr checkout, or `tests/`.

## Critical

### C1. The harness attaches the piloted alacritree to the developer's live herdr, not the throwaway

Claim in the spec: the alacritree child inherits the herdr variables, so every herdr process it spawns lands on the throwaway server, with no production change (section 4, "alacritree isolation").

Evidence. A shared-view attach runs `herdr session attach <name>` where `name` comes from `running_session_name`, which takes the first entry with `running: true` from `herdr session list --json` (`herdr/cli.rs:159-185`). herdr lists every session under its config dir with `default` first (`herdr/src/session.rs:187-212`), regardless of `HERDR_SESSION`. With the developer's default server up, the child attaches its rows to it. The seatbelts cover commands the harness runs, not commands the child spawns.

There is a second, opposite hole in the same paragraph. herdr's config dir on Windows is `%APPDATA%\herdr` (`herdr/src/config/io.rs:30-47`), and the child's `APPDATA` is the temp dir. Its herdr children therefore resolve `sessions/<name>/herdr.sock` under the temp dir, where a harness server started with the real `APPDATA` does not exist. As written, the child sees no server at all. `XDG_CONFIG_HOME` outranks `APPDATA` in herdr (`io.rs:31`), and `run_isolated` does not clear it (`tests/cli_isolation.rs:15-29`).

What the spec should say. The throwaway server and every harness herdr command run under the same redirected `APPDATA`, `LOCALAPPDATA`, `XDG_CONFIG_HOME` and `HOME` as the alacritree child, so herdr's session directory is the temp dir for all three parties. That makes the developer's socket invisible to `session list`, which is what closes the first hole, and puts the harness server where the child looks, which closes the second. `HERDR_SESSION` stays set so the session is named and `server stop` and `session delete` target it. Seatbelt one becomes: `session list --json` in the child's environment shows exactly one running session, named after the test, whose `session_dir` is under the temp dir.

### C2. Test 3 cannot reach the hold-off clock

Claim: `send-text` driving continuous input holds the follow off, and is the one test that catches a debounce wired to the wrong clock (Testing, item 3).

Evidence. `SendText` writes bytes to the PTY (`app.rs:11386-11398`). `last_input` moves only when egui reports events (`app.rs:11535-11537`). `RunAction` dispatches a binding action without an event either (`app.rs:11444-11449`). No CLI or IPC surface produces direct input, so the test as designed either fails to hold off and proves nothing, or passes only after making `SendText` touch the clock, which is a production change made for a test and misreports what the debounce is for.

What the spec should say. Either inject real input with Win32 `SendInput` into the focused window from the test (`windows-sys` is already a dependency; add `Win32_UI_Input_KeyboardAndMouse` for the test's use), or drop the hold-off from the end-to-end suite and rely on the unit tests in item 5. Say which.

### C3. The trail does not hear about focus moves alacritree itself makes, and has no guard against in-flight samples

Claim: recording and proposing happen together, so a focus alacritree itself asked for does not come back as a follow (section 2, rule 5).

Evidence. Three paths move herdr's focus without the trail seeing an edge it proposed: the `Focus` arm (`app.rs:1740-1751`), the attach gesture, whose `herdr_attach_gesture` calls `focus_pane` first (`herdr/cli.rs:241`), and the shared-view `Follow` (`app.rs:1752`, then `attached` at `1821`). Sequence under `"always"`: native session active, trail says `t1`; the user opens herdr row `t3` from the palette; the gesture focuses `t3` and the new session becomes active; the user switches back to the native session inside the poll interval; the sample lands saying `t3`; the trail sees `t1 -> t3` and, after the quiet gap, pulls the user back to the row they just left. The second half is the race the shipped path already defends against: a listing in flight when the gesture ran reports `t1` after the trail has been stamped `t3`, and the edge runs backwards. `next` rejects such samples with `sampled_at <= self.follow_after` (`view.rs:82`); the trail needs the same.

What the spec should say. A trail entry is `(terminal_id, stamped_at)`. `attached`, the `Focus` arm and the attach gesture stamp the trail with the key they targeted and `now` (`attached` takes only an id today, `view.rs:47`, so either it gains the key or `app.rs` stamps separately). A sample with `sampled_at <= stamped_at` cannot form an edge. Recording advances on every attentive frame for every side; proposing is the separate decision. "Edge-triggered rather than watermarked" is then half right: it is edge-triggered with a per-side watermark.

### C4. A follow the app cannot deliver consumes the edge and is never retried

Claim: the shared-view path is unchanged, and the trail path is edge-triggered so it declines to re-follow a pane the user has walked away from.

Evidence. `follow_herdr_view` returns silently when `herdr_session_name` is `None` (`app.rs:1808`), which it is until the background name read lands after the first successful poll (`poll.rs:283-288`, `308-317`, started at `364`), and when `herdr_row_workspace` is `None` (`app.rs:1797`). The shared-view path survives that because `focused != key` still holds on the next newer sample and it proposes again. The trail path has already recorded the new id, so the first follow after a server start under `"always"` races the name read and is lost for good.

What the spec should say. Either the trail records only after the app reports the follow was dispatched (`attached` is the natural signal), or an undeliverable follow becomes a pending attach the way `poll_herdr_attach` already handles a missing name by calling `running_session_name` on the pool (`app.rs:1638-1645`). The second reuses a shipped path and keeps the follow off the UI thread.

## Major

### M1. The pending follow's lifecycle is unspecified for five interleavings

Claim: three rules govern the pending follow: quiet gap, expiry, newest wins (section 2, "Holding off").

Evidence and gaps.

- The user switches sessions while a follow is pending. Nothing clears it. If they land on a shared-view session, rule 1 runs first and the two paths race for the same target.
- The pending target's session is closed. `closed(id)` knows ids (`view.rs:37-45`); the pending holds a key, so a follow to a row the user just closed respawns its attach client.
- The side stops answering. `agents` is cleared on a failed poll (`poll.rs:373-376`), so delivery finds no row and returns; the pending is consumed, which is fine, but the trail then compares against nothing (see M2).
- `attentive` goes false while pending. The spec says expiry and the inattentive gate do not interact. They do: the clock keeps running while the user is away, the follow expires, and the edge was recorded at proposal time, so on return nothing re-proposes it. That is the catch-up-on-return case the spec claims to preserve, lost for any change first seen just before the user left.
- Rule 6 is checked at edge time only. A pending follow whose target became the active session by the user's own gesture still fires `follow_herdr_view`, which calls `focus_terminal` and `attached` again.

What the spec should say. A pending follow is dropped when the active session changes or its target's session closes; its clocks count attentive time only; rule 6 is re-checked at delivery. Also say what `busy` does to the pending: `next` returns before the trail path while busy (`view.rs:74-76`), and an attach on Windows can hold busy for seconds.

### M2. What the trail records when no listed pane is focused is undefined, and the tests depend on it

Claim: the trail compares each side's currently focused terminal id against what it recorded.

Evidence. With `show_panes = false`, `agents()` carries only agent panes: unattached sides ask `agent list`, and attached sides retain `status.is_some()` (`poll.rs:260-262`, `399`). A new shell tab is invisible, and the focused id is `None` whenever herdr's focus sits on an unlisted pane. A failed poll empties the list. `Endpoints::adopt_running` drops a WSL cache when its distro stops and creates a fresh one when it starts (`poll.rs:563-580`), while a `Side`-keyed trail keeps the old entry.

What the spec should say. "No focused pane in the listing" and "side not answering" leave the trail entry untouched and never form an edge; a side whose cache disappears is forgotten; `Some` after `None` is first sight, not an edge. And the end-to-end config must set `show_panes = true`, or `tab create` produces a pane the child never lists.

### M3. `herdr tab create` does not focus the new tab

Claim: `tab create` on the test server moves alacritree to a row for the new pane (Testing, item 1).

Evidence. The CLI defaults `focus` to false (`herdr/src/cli/tab.rs:56`, `87-93`) and the server focuses only when asked (`herdr/src/app/api/tabs.rs:47-130`, the `if focus` branch at `116`). `pane list` reports a pane as focused only when its workspace is the server's active one, its tab is that workspace's active tab, and it is that tab's focused pane (`herdr/src/app/creation.rs:325-336`), so without `--focus` no edge forms and test 1 fails for the wrong reason.

What the spec should say. `tab create --focus`, or `tab create` followed by `tab focus`.

### M4. `last_input` is not the clock the spec describes

Claim: bare pointer motion does not count, or hovering would hold a follow off forever.

Evidence. `last_input` advances on any non-empty event list (`app.rs:11535-11537`), which includes `PointerMoved`, `MouseMoved`, `WindowFocused` and `Ime`. It is also the liveness probe's grace clock (`PROBE_GRACE`, `app.rs:470`, `1351`), so changing its semantics reaches the probe.

What the spec should say. A separate timestamp fed by the event kinds the spec lists, leaving `last_input` as is; or accept hover and say so. Note that `WindowFocused(true)` on return restarts the gap either way, which is acceptable but should be stated.

### M5. `follow_focus: Option<String>` publishes no schema default, and the tests the spec names do not exist on this branch

Claim: regenerate the schema, publishing `"herdr"` as the default; `config_schema` and `schema_defaults` both have to pass.

Evidence. `attach: Option<String>` publishes no `default` today (`schema/alacritree-config.json:457-464`); schemars emits a field default from the `Raw*` type's `Default` and skips `None`. `alacritree/tests/` holds `cli_isolation.rs` and `config_schema.rs` only; there is no `schema_defaults` test and no `schema-defaults-allowlist.txt`. They are on the stacked config-defaults PR the handoff mentions, not here.

What the spec should say. Pick one: mirror `attach` exactly and publish no default on this branch, or carry `"herdr"` through the `Raw*` `Default` as `AGENTS.md` asks, with a non-`Option` field. Name the tests that exist here, and note that the defaults test lands with the stack.

### M6. Nothing can report whether the window gained foreground

Claim: a window that never gained foreground fails with that message rather than passing quietly.

Evidence. Neither `ListSessions` nor any other reply carries window focus (`ipc.rs:67-125`, `app.rs:11494-11509`). The follow gate reads `viewport().focused` (`app.rs:1708`) and nothing exports it.

What the spec should say. The test asks Win32 `GetForegroundWindow` and `GetWindowThreadProcessId` and compares against the child pid; `windows-sys` with `Win32_UI_WindowsAndMessaging` is already a dependency (`alacritree/Cargo.toml:92-98`) and integration tests may use the crate's dependencies. Exporting focus over IPC would be the alternative, and a production change.

### M7. Teardown of the GUI child and the server child is unspecified

Claim: teardown runs `server stop` then `session delete` from `Drop`.

Evidence. The `Quit` action opens a dialog, it does not exit (`app.rs:3638-3640`). `herdr server` is `run_server` in the foreground (`herdr/src/main.rs:545-546`), so the harness holds a `Child`. `server stop` waits up to 15 s (`herdr/src/session.rs:14`, `247-297`) and `session delete` refuses a running session (`299-317`). `Child::kill` on Windows is `TerminateProcess`, which skips `Session::drop`.

What the spec should say. Order: `server stop` first so every attach client exits on its own, then kill and wait the GUI child, then kill the server child if `stop` timed out, then `session delete`. Say that TerminateProcess skips the shutdown message and that conpty children die with the pseudoconsole handle.

### M8. The WSL side is not abandoned, and a login profile can override the forwarded name

Claim: the WSL side looks for a session that does not exist, reports no herdr, and is abandoned for the run.

Evidence. A missing session yields `server_not_running` in herdr's own voice (`herdr/src/cli.rs:832-846`), which alacritree maps to `PollError::Server` (`herdr/cli.rs:212-217`). `Reach::abandoned` needs `Absent` (`poll.rs:40-42`), so the side is retried every 30 s (`poll.rs:14`). Harmless, but false. Separately, `Side::command` runs `sh -lc` (`herdr/cli.rs:33-42`), which sources the login profile; a profile exporting `HERDR_SOCKET_PATH` outranks the forwarded `HERDR_SESSION` (`herdr/src/session.rs:80-91`, `173-181`). That precedence is between environment variables only: an explicit `--session <name>`, and the rewritten `session attach <name>`, set a flag that is checked before `HERDR_SOCKET_PATH` is read (`session.rs:174-176`, `448-456`).

What the spec should say. The side answers `server_not_running` and keeps retrying on the recovery cadence; the loud-failure assertion is what covers a profile that overrides the name.

## Minor and wording

- W1. `session_json` is a free function over `&Session` (`app.rs:11494`). `agent` needs `herdr_backed_activity` (`app.rs:8623-8628`), `multiplexer` needs `find_herdr_agent` and `herdr_session_name`; all are `AlacritreeApp` methods. Say it becomes a method or takes the caches.
- W2. `pane_id` and `tab_id` do not go null on a failed poll if read through `attachment_pane`, which retains them with `current = false` (`poll.rs:168-171`, `416-422`). Name the lookup the field uses.
- W3. Seatbelt one's `sessions/<name>` substring is `sessions\<name>` on Windows (`herdr/src/session.rs:161-171`). Compare path components, or read `session_dir` from `session list --json`.
- W4. Rule 6 needs the unfiltered key: `next` drops a direct-attach `active` before anything else (`view.rs:66-67`), so the trail path would see no key for such a session and follow it to itself.
- W5. Test 1 should assert `current_workspace` too; `is_active_tab` is per workspace (`app.rs:11345-11349`).
- W6. "`follow_herdr_view` returns early unless `herdr_row_workspace` resolves" holds only when no session already holds the key (`app.rs:1794-1799`).
- W7. "Both must still hold exactly as written" cannot survive the `ViewInputs` signature; say the assertions hold, the calls change.
- W8. The trail path is unreachable in today's control flow: `next` returns at `active?` before reading any snapshot (`view.rs:77`). The restructure is implied; state it.
- W9. `next` is called before the pending `Focus` job is drained and its result is discarded on that frame (`app.rs:1710-1738`); consistent today because `busy` makes it `None`, and the pending follow's state must not advance on such a frame either.
- W10. The `[tasks.e2e]` block goes in `devkit.local.toml` in the main checkout, which is untracked and not in this worktree. Say where it lives.
- W11. nextest 0.9.143 accepts `--run-ignored ignored-only` and `--test-threads`; `cargo test` still compiles the target and runs zero tests. Fine, worth a sentence.

## Claims verified true

- A native session's snapshot comes out `None`: `app.rs:1694-1703`.
- `Side` derives `Hash`: `herdr/model.rs:10`. `HerdrKey` too, `model.rs:174`.
- `EndpointCache::session_name` exists and is `None` until the read lands: `poll.rs:283-288`.
- `&[EndpointCache]` is allocation-free: `caches()` returns a slice, `poll.rs:481`. Recording a changed id clones one `String`.
- Recording only while attentive preserves catch-up-on-return, with the exception in M1: a change seen just before leaving expires unseen.
- `LiveState::label` and `from_herdr` exist as described: `session.rs:168-186`.

## The unresolved questions

### 1. Does a herdr asked for a nonexistent session spawn a daemon?

From source: CLI subcommands go through `send_request` and map a connect failure to `server_not_running` with no spawn (`herdr/src/cli.rs:764-777`, `832-846`; `cli/status.rs:183`). A daemon is spawned only by `auto_detect_launch` (`herdr/src/main.rs:773-778`, `server/autodetect.rs:295-308`, spawning at `194`) and the remote host (`remote/host.rs:44`). `herdr session attach <name>` is rewritten by `configure_from_args` into a bare launch with an explicit session (`session.rs:35-52`), so it reaches `auto_detect_launch` and does spawn a daemon for a session that does not exist.

So `pane list`, `session list`, `status server` and `tab create` fail cleanly; `session attach` and bare `herdr` spawn. alacritree only runs listings on a WSL side until a pane is listed there, so the `WSLENV` mitigation leaves no daemon. The orphan seen during probing most likely came from a `session attach` or a bare `herdr`. Reverified against `120c6820`.

### 2. What quiet gap and what expiry?

Not derivable from the code. The codebase's one precedent for "the user is active" is `PROBE_GRACE = 10 s` (`app.rs:470`). A gap of about 750 ms clears the pause between keystrokes; an expiry of about 10 s matches the precedent. Both counted in attentive time (M1). The gap is measured from `last_input`, not from first sight, so a follow lands at the later of first sight and `last_input + gap`.

## What changes the design versus wording

Design: C1, C2, C3, C4, M1, M2, M5, M6, M7. Test plan only: M3, M8. Wording: M4 (if hover is accepted), W1 to W11.
