# OSC passthrough: the codes that need no grid position

Issue: https://github.com/AbysmalBiscuit/alacritree/issues/90
Parent survey: https://github.com/AbysmalBiscuit/alacritree/issues/54

## Problem

alacritree inherits its whole OSC surface from `alacritty_terminal` and `vte`. That covers the classic set, meaning title, palette, hyperlinks, clipboard write and cursor shape, and nothing modern. There is no OSC 7, no notifications, no progress reporting, and OSC 52 read is refused before an event is ever emitted.

`vte::ansi::Processor` matches a fixed set of OSC numbers and drops the rest with a log line, so a sequence can be parsed upstream and still do nothing here. `Term` never overrides `Handler::set_mouse_cursor_icon`, so OSC 22 reaches an empty default. Neither gap can be closed by editing alacritree, because the decision happens inside a crate this fork treats as read-only.

The most valuable of these is OSC 7. alacritty's own cwd probe lives in its GUI crate rather than in `alacritty_terminal`, so a plain alacritree session has no cwd signal at all. The herdr integration reads `foreground_cwd`, which covers herdr panes and nothing else. A worktree-aware sidebar that wants to know where a shell actually is has no other clean signal.

## Scope

In: OSC 52 read, OSC 7, OSC 9;9, OSC 9 and OSC 777 notifications, OSC 9;4 progress, OSC 22 pointer shape.

Out: OSC 133 and OSC 9;12, both prompt marks that must be recorded against the row the cursor is on when the sequence arrives. That needs the parse loop, not a byte tap, and gets its own issue.

Every item changes visible behaviour, so every item is gated by a config key whose default is the behaviour alacritree has today.

## Why a byte tap

`vte::Parser` and `vte::Perform` are the crate's low-level API, and `alacritty_terminal` re-exports them as `alacritty_terminal::vte`. A second `Parser` fed the same bytes, with a `Perform` that implements only `osc_dispatch`, receives every OSC the `ansi` layer drops. No fork of `vte`, no edit to a vendored crate.

Two claims in the originating issue turned out to be wrong and are corrected here. `vte`'s 1024-byte OSC cap applies only to `no_std` builds: with the `std` feature, which `alacritty_terminal` enables, `osc_raw` is a plain `Vec` and the const generic on `Parser` is documented as unused. There is nothing to widen, and `Parser::new()` is all the tap needs. Separately, the issue assumed the tap would roughly double VT parsing work on the PTY read thread. It would, and that is why the parse does not run there.

## Measured cost

A throwaway benchmark compared four strategies against three synthetic streams, timing only what the PTY read thread pays. Microseconds per MiB, 8 KiB reads, best of five. This table is a snapshot from one machine and exists to justify the choice, not to be maintained.

| strategy | plain build log | colored log | TUI redraw |
|---|---|---|---|
| terminal's own parse, for scale | 940 | 1981 | 3293 |
| handoff to another thread, send side only | 56 | 67 | 71 |
| parse inline, skipping to each escape byte | 24 | 1544 | 3476 |
| parse inline, every byte | 752 | 2290 | 3100 |

Escape density decides everything. Skipping to each escape byte is nearly free on plain output and costs more than the terminal's own parse once escapes arrive every few bytes, which is exactly when a TUI is repainting and latency is visible. Handing the bytes to another thread stays flat because it never parses on the read thread. It loses on plain output by 32 microseconds per MiB, which is three percent of what the terminal already spends there, and wins on a redraw by 3.4 milliseconds per MiB.

At 64 KiB reads the handoff figure rises to between 151 and 222 microseconds, which is the per-chunk allocation showing up. Buffers are pooled for that reason.

To reproduce: build a small binary against `vte` with default features and `memchr`, generate streams at three escape densities, and time chunked feeds of each strategy.

## Architecture

Two new modules in `alacritree/src/`. `pty_tee.rs` moves bytes, `osc_tap.rs` reads them. Neither knows the other's concerns.

### The tee

`TeePty<P>` is generic over `P: EventedPty + OnResize`. It forwards `register`, `reregister`, `deregister`, `writer`, `next_child_event` and `on_resize` untouched, and returns a `TeeReader<P::Reader>` from `reader()`. On Windows it wraps `RearmingPty`, elsewhere it wraps `Pty` directly. This is the same wrapper shape `pty_rearm.rs` already runs in production, one layer out.

`TeeReader::read` calls the inner read, copies the filled slice into a buffer taken from the pool, and sends it. That is the entire read-thread cost.

The wrap is unconditional. `EventLoop::new` is generic over the PTY type, so a conditional wrap would need two spawn paths with different concrete types. `TeePty` always wraps and holds an `Option` tap handle, which is `None` when every `[vt]` key is off. In that state the cost is one branch per read.

### The tap thread

One thread per session, started only when at least one `[vt]` key is on, exiting when its sender drops with the tee. No registry, no teardown ordering.

It owns a `vte::Parser` and a `Perform` that implements only `osc_dispatch`, leaving vte's empty defaults everywhere else. Recognised sequences become `OscEvent` values sent over an `mpsc::Sender` followed by `egui::Context::request_repaint`, the same contract `EventProxy` uses.

Flow control runs both ways. The read thread sends over a bounded `sync_channel`; a full queue drops that chunk rather than blocking, and the next successful send carries a gap flag that resets the parser, because a hole in the byte stream makes framing meaningless. A return channel recycles buffers: the read thread takes one with `try_recv` and allocates when the pool is empty, and a failed return simply drops the buffer. Both directions are non-blocking on the read side, so neither can deadlock, and buffers in flight are bounded by the channel capacity.

### Reaching the UI

`Session` holds the receiver and drains it in `drain_events`, after the existing terminal-event loop, folding results into the same `DrainOutcome`. Parsing stays off the UI thread. Every decision that touches app state stays on it, where it already is.

### Dispatch

vte splits OSC parameters on semicolons before `osc_dispatch` sees them, so any payload that can legally contain a semicolon is reconstructed by rejoining the trailing parameters. That applies to OSC 7 paths, 9;9 paths, notification bodies and OSC 777 bodies.

OSC 9 carries two unrelated protocols. ghostty's `osc9.zig` resolves them by trying ConEmu first and requiring the payload to be a digit run followed by a semicolon, falling through to the iTerm2 notification otherwise. alacritree copies that rule, so `9;9;C:\src` is a cwd report and `9;9 items remaining` is a notification.

The tap classifies through one pure function taking vte's raw parameter slices and returning an optional `OscEvent`. That seam is what makes every protocol decision testable without a PTY, a thread or an egui context, matching what `sidebar_nav.rs`, `git_nav.rs` and `row_label.rs` already do.

## Per-code behaviour

### OSC 52 read

No tap involvement. `term_config` starts passing the configured `Osc52` value instead of leaving the default, and `apply_term_event` gains an arm for `Event::ClipboardLoad`.

The event carries a formatter that builds the whole reply from the clipboard text. `drain_events` must stay free of OS clipboard access, as its existing comment on `DrainOutcome` requires, so the formatter is carried out to the caller the way copied text already is. `app.rs` reads the clipboard beside the existing `clipboard::write` call, runs the formatter, and writes the bytes back.

Upstream alacritty answers the read only when the window is focused, a guard beyond the config key. The faithful translation here is window focused and session visible, since a background alacritree session is what a background alacritty window would be.

### OSC 7 and OSC 9;9

Both land on a new `Session` field. They must not touch `working_directory`, which is the workspace key: `app.rs` compares it directly against `WorkspaceKey` values to decide which sidebar workspace a session belongs to, so writing a report into it would re-home sessions on every `cd`.

OSC 7 arrives as a file URL. A non-`file` scheme is rejected. A host is accepted when it is empty, `localhost`, or this machine's name compared case-insensitively, and rejected otherwise. The path is percent-decoded, and a Windows drive path arriving as `/C:/src` loses its leading slash. An empty payload is an explicit reset rather than a malformed report, following ghostty.

ghostty's own comment states the hazard better than a paraphrase would:

> OSC 7 is a little sketchy because anyone can send any value from any host (such an SSH session). The best practice terminals follow is to valid the hostname to be local.

Read that comment for what it is, though. The check filters *honest* shells: fish, vte.sh and Terminal.app's integration all interpolate a hostname, so a remote one announces itself. Anything writing to the PTY with intent writes `localhost` and passes. It is a correctness filter for cooperating shells, not a trust boundary, and the design must not lean on it as one.

ghostty also rejects a missing host. alacritree accepts it, because an empty host is unspecified rather than remote and rejecting it would break hostless integrations while stopping nobody, since a spoofer writes `localhost` instead.

OSC 9;9 carries a bare path and no host at all. ConEmu documents the payload as a quoted string; Windows Terminal's `DoConEmuAction` strips surrounding quotes when both are present and parses the bare string when they are not, then validates the result before storing. alacritree does the same. Windows Terminal splits on semicolons and truncates a path containing one, so the remainder is kept instead.

Windows Terminal is the only reference that accepts a bare 9;9, and it is the right precedent here because this is a Windows-first fork. ghostty is not: `osc9.zig` routes 9;9 to the same command as OSC 7, but `reportPwd` then parses the value as a URI, so a bare path fails for want of a scheme, and the whole function returns early on Windows regardless.

### The guard that actually holds

Since the host check stops nobody who means harm, the guard belongs on the value rather than on its origin, and it applies identically to OSC 7 and OSC 9;9 once each has been decoded.

The path must be rooted for the shell's platform: `/`-rooted for a Unix or WSL shell, `X:\` or `X:/`-rooted for a Windows shell. Everything else is dropped, which covers relative paths, `~`, drive-relative `C:foo`, and above all any path beginning `\\` or `//`.

That last prefix is the reason this rule exists. A payload of `\\evil\share`, or an OSC 7 path that percent-decodes to `//evil/share`, is a UNC path on Windows. Touching it with `is_dir` opens an SMB connection to a host the payload named, which is the NTLM-hash-leak class, and a session spawned there runs `git` against the attacker's `.git/config`. The sender need not be a remote shell at all: `cat` of a downloaded file, a commit message in `git log`, or a build artifact through `less` all reach the PTY. Windows Terminal's `is_legal_path` is a character filter and does not reject UNC, so it does not cover this.

The rule runs before WSL translation, not after. `wsl::linux_to_windows` produces a `\\wsl.localhost\<distro>\...` UNC path of its own, and that one is trustworthy precisely because the distro comes from `Session::wsl_distro` rather than from the payload. Checking after translation would reject our own legitimate output.

The new `Session` field is documented as attacker-choosable: any process whose bytes reach the PTY can set it, local or remote. Consumers guard at use. Hover text displays it as-is, since a string in a tooltip protects nothing. The sibling spawn filters on `is_dir` and falls back to the workspace directory, which costs one stat on the UI thread at the moment the user clicks and turns a failed spawn back into the behaviour that exists today. kitty does the same fallback through `cwd_of_child`.

Consumers on this branch are the sidebar row's hover text and the directory a sibling session spawns into. Re-homing a session into the workspace its reported cwd matches is deliberately not included: it is the payoff the parent issue points at, and a `cd` into a temporary directory and back reshuffling the sidebar deserves its own design round.

Re-homing also raises the stakes from one failed spawn to a session listed under the wrong worktree, holding that workspace's active-session slot and driving its git panel. The prefix match against known local project roots is itself a strong filter, since a Linux path on Windows matches nothing, but a remote host laid out like the local one with the same username and clone paths would slip through. That is where kitty's `child_is_remote` earns its cost, and the re-homing issue should add it by extending the per-platform foreground probe already in `session.rs` with a client name list and refusing to re-home while one is in the foreground. Adding it on this branch to protect a hover string would be more than the job needs.

### OSC 9 and OSC 777

Both are notifications carrying application-supplied text, and both join the attention pipeline that bells and spinner titles already use. Reusing it needs two changes, because every existing trigger is payload-free and the pipeline is built on that assumption.

**The text is stored in the drain, not in the attention path.** `drain_events` writes the body onto the session before any visibility test runs, so a session that is visible and focused keeps the text even though the attention block skips it and no toast fires. The sidebar row's hover text shows the latest body, and the next notification replaces it. This also covers the ordinary case, where the toast fires frames later once the grace window elapses and the text has to outlive the outcome that carried it.

**An explicit notification bypasses the transition latch.** `DrainOutcome` carries the body alongside the attention flag, which is what lets the pipeline tell an explicit notification from an inferred one. Today the toast fires only on the rising edge of `needs_attention`, so that a bell and a spinner title in one idle cycle produce a single toast. A background session stays latched until you look at it, so without this change an application sending "build started" and then "build failed" would get one toast. The toast gate becomes the transition **or** an explicit notification.

The debounce is deliberately left alone. An explicit notification still goes through `poll_attention_debounce`, so a spinner-shaped title still cancels it. That is inert at the shipped default, where `attention_grace_ms` is zero and zero grace fires before the spinner check, and a user who sets a grace window asked for debouncing. Within a non-zero grace window, notifications arriving while one is already pending coalesce to the last body, since `pending_attention` is a single slot.

`notify_attention` uses the stored body instead of deriving one from the working directory. Bodies are truncated before reaching the platform notifier, which has its own limits.

### OSC 9;4

The ConEmu progress report. Windows Terminal's `DoConEmuAction` is the reference: a state above four rejects the whole sequence, a progress value above one hundred clamps, and an empty state field means zero.

The state is stored on the session and `session_row` paints a bar across the row, filled to the percentage, with error and paused carried by its colour and indeterminate filling it whole. This needs no addition to the baked glyph set.

### OSC 22

The pointer shape name maps to an `egui::CursorIcon`, is stored on the session, and `terminal_view` applies it while the pointer is over the grid. An unrecognised name leaves the current shape alone, because not understanding a request is not a request to reset.

## Configuration

OSC 52 read needs no new key. Upstream alacritty already has `[terminal] osc52` taking the `Osc52` enum and defaulting to `OnlyCopy`, and `Term` already enforces it. Mirroring upstream means parsing that exact key from the shared `alacritty.toml` and keeping that default, so a read stays refused unless the user opts in. Refusing by default is also where wezterm and Windows Terminal sit, and an app that can read the clipboard can read whatever the user last copied anywhere else.

Everything else lives in a new `[vt]` table in `alacritree.toml`, with keys named for behaviour rather than sequence number: `report_cwd` for OSC 7 and OSC 9;9, `notify` for OSC 9 and OSC 777, `progress` for OSC 9;4, and `pointer_shape` for OSC 22. Each doc comment names the sequences its key governs, and those comments become the published schema's hover text, so searching the schema for a sequence number still finds the key.

`notify` is deliberately not named `notifications`, because `[ui] notifications` already gates whether a desktop toast fires at all. The two compose rather than duplicate: `[vt] notify` decides whether an OSC 9 or 777 sequence reaches the attention pipeline, and `[ui] notifications` decides whether anything reaching that pipeline becomes a toast. With the first on and the second off, the sidebar row highlights and nothing pops up.

Every key defaults to false, which is today's behaviour. There is no config hot-reload in this tree, so the enabled set is fixed when a session spawns and a change needs a restart, the same as the transparency flag.

The schema is regenerated with `ALACRITREE_UPDATE_SCHEMA=1` and the `config_schema` and `schema_defaults` tests must pass, with any allowlist change justified against omission behaviour.

## Error handling and bounds

Nothing here may break a session. Every failure discards the sequence and logs at debug. If the tap thread cannot be spawned, the tee runs without one and the session opens normally, the way the existing notifier already handles a backend that will not start.

Parse failures discard rather than reset, leaving the previous value in place. Only an explicitly empty OSC 7 payload clears the reported cwd, and only progress state zero clears progress.

`osc_raw` is unbounded under the `std` feature, so an unterminated OSC sequence grows it until memory runs out. `Term`'s own parser already carries that exposure and the tap would double it, so the tap counts bytes fed since its last dispatch and resets its parser past a fixed ceiling. That costs one counter and closes the hole being added.

The hostname resolves once into a `OnceLock`. If the lookup fails, only an empty host and `localhost` count as local.

A reported path that does not exist is still stored. The tap reports what the shell said; it does not stat the filesystem on every `cd`. The stat happens once, in the sibling-spawn consumer, at the moment the user clicks.

## Testing

The classification seam carries the bulk, table-driven and free of PTYs, threads and egui. Host policy across empty, `localhost`, the machine name in the wrong case and a foreign host. A non-`file` scheme. Percent escapes. A Windows drive path arriving with a leading slash. An empty OSC 7 payload clearing the value. For 9;9, quote stripping, a bare unquoted path, and a path containing a semicolon surviving the parameter rejoin. For 9;4, a state above four rejecting, progress clamping, an empty state meaning zero. For OSC 9, the disambiguation itself in both directions.

The rooted-path rule gets its own table, since it is the one rule here with a security consequence. Both spellings of the UNC prefix, reached by both routes: a 9;9 payload of `\\evil\share`, and an OSC 7 URL whose path percent-decodes to `//evil/share`. A relative path, a `~` path, and a drive-relative `C:foo`. A legitimate `\\wsl.localhost\...` produced by `wsl::linux_to_windows` surviving, which pins the ordering: the rule runs on the payload, translation runs after.

The tee gets the test that matters most: bytes in equal bytes out, across chunk boundaries and short reads. A tee that drops or duplicates a byte corrupts the grid in a way that would be miserable to trace back here.

The tap thread gets two: a sequence split across chunks still dispatches, and a gap flag discards the partial sequence rather than mis-framing what follows.

The two attention-pipeline changes each get one, since both fix behaviour the existing triggers depend on and a regression in either would be silent. A visible, focused session keeps the notification body even though no toast fires. And two explicit notifications to an already-latched background session produce two toasts, where two bells still produce one.

Two paths get end-to-end tests through a real PTY, which `session.rs` already spawns in tests. OSC 7 writes the real sequence into a real session and asserts the reported cwd lands. OSC 52 read gets refused-by-default and answered-when-opted-in, modelled on the existing OSC 52 copy test.

Config keys default to false and parse, following the existing `config.rs` patterns.

Deliberately untested: the platform notifier, which has no seam; the progress bar's pixels; and performance, because the parse no longer runs on the read thread, so a future `vte` regression cannot land on the hot path and a benchmark in the tree would be maintenance for a number that stopped mattering.

One risk for the plan to verify rather than assume: `steady_state.rs` asserts the sidebar's per-frame reconcile is allocation-free on an unchanged frame. If the reported cwd reaches the sidebar snapshot, that assertion is what will catch it.

## Known consequences

- A spinner-shaped title cancels a pending notification, because an explicit notification still goes through the existing debounce. Inert at the shipped default, since zero grace fires before the spinner check runs.
- Within a non-zero grace window, notifications arriving while one is pending coalesce to the last body.
- The reported cwd is choosable by anything that writes to the PTY, local or remote. The host check only filters shells that honestly name a remote host. Safety comes from the rooted-path rule at parse time and from consumers guarding at use, not from knowing who sent the sequence.
- A remote shell with a hostless integration, or one emitting 9;9, still reports a path alacritree accepts. On this branch that surfaces as a hover string naming a path that is not here, and a sibling spawn that falls back to the workspace directory.

## Unverified

Two claims behind the reasoning above are recalled rather than checked against a pinned checkout. Neither changes the policy, since the rooted-path rule holds either way.

- fish's default OSC 7 payload interpolating the hostname, and WSL defaulting its hostname to the Windows computer name. If both hold, a `wsl` invocation from an unshimmed PowerShell session passes the host check and reports a Linux path on a Windows machine, which is the most likely way a user meets the fallback.
- Whether a percent-decoded `//x` is what Rust's `is_dir` hands to the Windows API as UNC. Rejecting the prefix is correct whether or not it normalises.

## Open questions

None blocking. Two deferred by choice: re-homing a session into the workspace its reported cwd matches, and driving the Windows taskbar from OSC 9;4 alongside the sidebar bar.
