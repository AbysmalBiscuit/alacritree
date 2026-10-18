# OSC Through vte Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the OSC tap (`pty_tee` + the `osc_tap` thread) with one generic hook through which `vte` and `alacritty_terminal` hand alacritree every OSC sequence they do not interpret, plus OSC 22, which `vte` parses but `Term` ignores.

**Architecture:** `vte` is vendored at `vte/` and gains one `Handler` method, `unhandled_osc`, called from the catch-all arm of `Performer::osc_dispatch`. `Term` implements it and `set_mouse_cursor_icon` by sending two new `Event` variants. alacritree's `EventProxy` intercepts both on the PTY thread, runs them through an `OscFilter` (the existing `classify` plus the repeat filter), and sends `OscEvent`s on the channel `Session::drain_events` already drains. The tee, the tap thread, its queue, its buffer pool and its scanner are deleted.

**Tech Stack:** Rust, `vte` 0.15.0 (vendored), `alacritty_terminal` (vendored), `cursor-icon` 1.x (re-exported as `vte::ansi::cursor_icon`), egui.

**Spec:** No spec document. The decision and its reasoning come from the session that produced PR #383 (`325-feat-vt-osc-codes`): a generic hook over a second parser, because future OSC support needs stream order, grid position and ordered replies, which a tap on another thread cannot give.

## Global Constraints

- Work lands on branch `325-feat-vt-osc-codes` in `C:/Users/Lev/Git/github/alacritree-worktrees/325-feat-vt-osc-codes` and goes up as more commits on PR #383.
- The vendored diff is only what Tasks 1 and 2 name. Anything about which OSC numbers matter, config gating, or what an event means lives in alacritree.
- Run build, test, lint and format through devkit: `devkit run task check|test|clippy|fmt --dir <worktree>`. Nightly fmt also rewrites unrelated files (`app/catalog*`, `tasks/view/preview.rs`); restore those with `git -C <worktree> restore <paths>` before committing.
- Commit with `devkit run task commit --dir <worktree> --arg "files=..." --arg "commit_subject=..." --arg "commit_body=..." --arg "coauthors=Claude Opus 5.5 <noreply@anthropic.com>"`. Conventional Commits, subject at most 72 chars, body wrapped at 72.
- Comments follow the unslop rules in `AGENTS.local.md`: no em dashes, timeless, no change narration.
- All `[vt]` keys stay off by default. With every key off, an unhandled OSC must cause no channel send and no repaint.

## Review Focus

- Default config (every `[vt]` key off) with a shell sending OSC 133 on every prompt: nothing is queued and the UI is not woken. Test in Task 3.
- A hidden session sending the same OSC 9;4 progress value repeatedly: one wake per changed value, none for repeats. Test in Task 3.
- OSC 22 with an X11 alias name (`hand2`, `xterm`, `left_ptr`): `vte` rejects the name and the pointer stays as it was. Test in Task 3.
- An OSC ended by CAN or SUB instead of BEL or ST: `vte` still dispatches it, so the hook fires with `bell_terminated == false`. Test in Task 2.

---

### Task 1: Vendor vte with an `unhandled_osc` hook

**Files:**
- Create: `vte/` (a copy of `~/.cargo/registry/src/index.crates.io-*/vte-0.15.0/`, without `Cargo.lock` and `.cargo_vcs_info.json`)
- Modify: `vte/src/ansi.rs` (the `Handler` trait near line 704, the `_ =>` arm of `osc_dispatch` near line 1528, the `tests` module at line 2017)
- Modify: `Cargo.toml` (root, `[patch.crates-io]`)
- Modify: `AGENTS.md` (the "Repository layout" bullets)
- Modify: `Cargo.lock`

**Interfaces:**
- Produces: `vte::ansi::Handler::unhandled_osc(&mut self, params: &[&[u8]], bell_terminated: bool)`, a default no-op. Called only for OSC sequences whose first parameter matches no arm of `osc_dispatch`. `params` are `vte`'s own `;` split (at most 16 entries).

- [ ] **Step 1: Copy the crate and wire the patch**

Copy the registry directory to `vte/`. Add under `[patch.crates-io]` in the root `Cargo.toml`, below `egui-winit`:

```toml
# Vendored vte whose `Handler` receives the OSC sequences its `ansi` layer
# does not interpret, so alacritree can read them from the terminal's own
# parse.
vte = { path = "vte" }
```

- [ ] **Step 2: Confirm the patch applies and vte stays out of the workspace**

Run: `devkit run task check --dir <worktree>`, then `cargo metadata --manifest-path <worktree>/Cargo.toml --no-deps --format-version 1 | jq -r '.workspace_members[]'`
Expected: check passes; `Cargo.lock` shows `vte 0.15.0` with no `source` line; `vte` is not a workspace member, as `egui-winit` is not.

- [ ] **Step 3: Write the failing test in `vte/src/ansi.rs` `tests`**

A `Handler` that records `unhandled_osc` calls, fed through `Processor::<StdSyncHandler>::advance`:

```rust
#[test]
fn an_uninterpreted_osc_reaches_the_handler() {
    // ESC ] 7 ; file:///tmp BEL
    assert_eq!(record(b"\x1b]7;file:///tmp\x07"), vec![(vec![b"7".to_vec(), b"file:///tmp".to_vec()], true)]);
    // ESC ] 9 ; 4 ; 1 ; 50 ST
    assert_eq!(record(b"\x1b]9;4;1;50\x1b\\"), vec![(vec![b"9".to_vec(), b"4".to_vec(), b"1".to_vec(), b"50".to_vec()], false)]);
    // A sequence vte interprets never reaches the hook.
    assert!(record(b"\x1b]0;title\x07").is_empty());
}
```

`record(bytes) -> Vec<(Vec<Vec<u8>>, bool)>` is a test helper returning the recorded calls.

- [ ] **Step 4: Run it and watch it fail**

Run: `cargo nextest run --manifest-path <worktree>/vte/Cargo.toml --features ansi an_uninterpreted_osc_reaches_the_handler`
Expected: compile failure, `unhandled_osc` is not a member of trait `Handler`.

- [ ] **Step 5: Add the method and call it**

In `Handler`: `fn unhandled_osc(&mut self, _params: &[&[u8]], _bell_terminated: bool) {}`, with a one-line doc comment. In `osc_dispatch`, make the catch-all arm call `unhandled(params)` and then `self.handler.unhandled_osc(params, bell_terminated)`. Leave every other arm alone.

- [ ] **Step 6: Run the test and watch it pass**

Same command as Step 4. Expected: PASS.

- [ ] **Step 7: Document the vendored crate in `AGENTS.md`**

Add a bullet after the `egui-winit/` one: `vte/` is a vendored `vte` whose `ansi::Handler` gains `unhandled_osc`, wired in through `[patch.crates-io]`. Nothing else in it changes.

- [ ] **Step 8: Commit**

Subject: `build(vte): vendor vte and pass uninterpreted OSC to the handler`. Files: `vte`, `Cargo.toml`, `Cargo.lock`, `AGENTS.md`.

### Task 2: Send the hook and OSC 22 as `alacritty_terminal` events

**Files:**
- Modify: `alacritty_terminal/src/event.rs` (`Event` and its `Debug` impl)
- Modify: `alacritty_terminal/src/term/mod.rs` (`impl<T: EventListener> Handler for Term<T>` at line 1059, plus its `tests` module)
- Modify: `alacritty/src/event.rs:1929` (the exhaustive no-op arm, so the vendored GUI still compiles)

**Interfaces:**
- Consumes: `vte::ansi::Handler::unhandled_osc` from Task 1.
- Produces:
  - `Event::UnhandledOsc { params: Vec<Vec<u8>>, bell_terminated: bool }`
  - `Event::MouseCursorIcon(crate::vte::ansi::cursor_icon::CursorIcon)`

  `Term` sends each from inside its parse, in stream order with every other event.

- [ ] **Step 1: Write the failing test in `term/mod.rs` `tests`**

Use an `EventListener` that pushes into an `Arc<Mutex<Vec<Event>>>`, and feed bytes with `Processor::<StdSyncHandler>::new().advance(&mut term, bytes)`:

```rust
#[test]
fn uninterpreted_osc_and_pointer_shape_reach_the_listener() {
    // ESC ] 7 ; file:///tmp BEL
    assert!(matches!(events(b"\x1b]7;file:///tmp\x07").as_slice(),
        [Event::UnhandledOsc { params, bell_terminated: true }] if params == &[b"7".to_vec(), b"file:///tmp".to_vec()]));
    // CAN ends the sequence without a terminator.
    assert!(matches!(events(b"\x1b]7;file:///tmp\x18").as_slice(),
        [Event::UnhandledOsc { bell_terminated: false, .. }]));
    assert!(matches!(events(b"\x1b]22;pointer\x1b\\").as_slice(),
        [Event::MouseCursorIcon(CursorIcon::Pointer)]));
    assert!(!events(b"\x1b]0;t\x07").iter().any(|e| matches!(e, Event::UnhandledOsc { .. })));
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo nextest run -p alacritty_terminal uninterpreted_osc_and_pointer_shape_reach_the_listener`
Expected: compile failure, no variant `UnhandledOsc`.

- [ ] **Step 3: Add the variants, their `Debug` arms and the two `Handler` methods on `Term`**

- **Doc comments.** `UnhandledOsc`: an OSC sequence the parser does not interpret, split on `;`. `MouseCursorIcon`: OSC 22, the pointer the application asked for.
- **`Debug` arms.** `UnhandledOsc` prints its params lossily as UTF-8; `MouseCursorIcon` prints the icon.
- **`Term` methods.** `unhandled_osc` copies the params into owned `Vec`s and sends the event; `set_mouse_cursor_icon` sends the icon.
- **`alacritty` GUI.** Add both variants to the no-op arm at `alacritty/src/event.rs:1929`.

- [ ] **Step 4: Run the test and watch it pass**

Same command as Step 2. Expected: PASS. Then `devkit run task check --dir <worktree>` passes.

- [ ] **Step 5: Commit**

Subject: `feat(terminal): report uninterpreted OSC and OSC 22 as events`. Files: `alacritty_terminal/src/event.rs`, `alacritty_terminal/src/term/mod.rs`, `alacritty/src/event.rs`.

### Task 3: Classify OSC events in `EventProxy` and delete the tap

**Files:**
- Rename: `alacritree/src/osc_tap.rs` -> `alacritree/src/osc.rs` (`git mv`)
- Delete: `alacritree/src/pty_tee.rs`
- Modify: `alacritree/src/lib.rs` (module list)
- Modify: `alacritree/src/session.rs` (`EventProxy` at lines 28-95, `OpenRequest.tap` at 715, the `TeePty` wrap at 806, `Session::pending` at 1037-1120, tap-driven tests at 1928-2115)
- Modify: `alacritree/src/app.rs:9488,9508` (callers of the renamed WSL test helper)
- Modify: `alacritree/src/config.rs:190` (the `any_enabled` doc names the tap thread)
- Modify: `alacritree/Cargo.toml` (`polling` back under `[target.'cfg(windows)'.dependencies]` with `origin/master`'s comment; drop `memchr` if `rg -l "memchr::" alacritree/src` is empty)
- Modify: `Cargo.lock`

**Interfaces:**
- Consumes: `Event::UnhandledOsc` and `Event::MouseCursorIcon` from Task 2.
- Produces, in `crate::osc`:
  - Kept unchanged: `ShellPlatform`, `Progress`, `OscEvent`, `local_hostname()`, `classify(params: &[&[u8]], policy: &OscPolicy) -> Option<OscEvent>`. `classify` loses its `b"22"` arm and `cursor_icon()`.
  - `OscPolicy` is `TapPolicy` renamed; same fields.
  - `pub(crate) struct OscFilter` with:
    - `new(policy: OscPolicy) -> Option<Self>`, returning `None` when `!policy.vt.any_enabled()`.
    - `unhandled(&mut self, params: &[Vec<u8>]) -> Option<OscEvent>`, which is `classify` plus the repeat filter.
    - `pointer_shape(&mut self, icon: CursorIcon) -> Option<OscEvent>`, gated on `policy.vt.pointer_shape` and passed through the repeat filter.
  - `fn egui_cursor(icon: CursorIcon) -> egui::CursorIcon`. `CursorIcon` is `#[non_exhaustive]`, so the fallback arm maps to `egui::CursorIcon::Default`.
- Produces, in `session.rs`:
  - `EventProxy::with_osc(self, filter: OscFilter) -> (Self, mpsc::Receiver<OscEvent>)`. The filter and sender are shared by every clone of the proxy.
  - `Session.osc_events` keeps its type and `drain_events` keeps its OSC loop.

- [ ] **Step 1: Move the repeat filter into `OscFilter` and write its tests in `osc.rs`**

- **Move.** Carry `last_cwd`, `last_progress`, `last_pointer_shape` and `is_repeat` over from `Sink` unchanged.
- **Delete.** `MAX_OSC_LEN`, `classified`, `ScanState`, `Destination`, `Sink`, `spawn`, and the stream tests that fed `Sink`.
- **Tests.** Port `a_repeated_state_is_emitted_once_and_a_repeated_notification_every_time` to call `OscFilter::unhandled` with split params, and add:

```rust
#[test]
fn a_filter_exists_only_when_a_vt_key_is_on() {
    assert!(OscFilter::new(OscPolicy { vt: VtConfig::default(), ..policy(ShellPlatform::Unix) }).is_none());
}

#[test]
fn the_pointer_follows_the_css_name_vte_parsed() {
    assert_eq!(egui_cursor(CursorIcon::Pointer), egui::CursorIcon::PointingHand);
    assert_eq!(egui_cursor(CursorIcon::Text), egui::CursorIcon::Text);
    assert_eq!(egui_cursor(CursorIcon::NotAllowed), egui::CursorIcon::NotAllowed);
    assert_eq!(egui_cursor(CursorIcon::EwResize), egui::CursorIcon::ResizeHorizontal);
}
```

Replace `osc22_maps_known_names_and_ignores_the_rest` with the test above.

- [ ] **Step 2: Write the failing proxy tests in `session.rs` `tests`**

```rust
#[test]
fn an_uninterpreted_osc_with_no_vt_key_on_is_dropped_without_a_wake() {
    let repaint = Recorder::default();
    let (proxy, events) = EventProxy::new(repaint.clone());
    proxy.send_event(TermEvent::UnhandledOsc { params: vec![b"133".to_vec(), b"A".to_vec()], bell_terminated: true });
    assert!(events.try_recv().is_err());
    assert_eq!(repaint.wakes(), 0);
}

#[test]
fn a_hidden_session_wakes_once_per_changed_progress() {
    let repaint = Recorder::default();
    let (proxy, osc) = EventProxy::new(repaint.clone()).0.with_osc(OscFilter::new(all_on_policy()).unwrap());
    proxy.set_visible(false);
    let progress = |v: &str| TermEvent::UnhandledOsc {
        params: vec![b"9".to_vec(), b"4".to_vec(), b"1".to_vec(), v.as_bytes().to_vec()],
        bell_terminated: true,
    };
    proxy.send_event(progress("50"));
    proxy.send_event(progress("50"));
    proxy.send_event(progress("60"));
    assert_eq!(osc.try_iter().count(), 2);
    assert_eq!(repaint.wakes(), 2);
}
```

`all_on_policy()` is a test helper building an `OscPolicy` with every `[vt]` key on.

- [ ] **Step 3: Run them and watch them fail**

Run: `devkit run task test --dir <worktree>`
Expected: compile failure, no method `with_osc`.

- [ ] **Step 4: Implement `EventProxy::with_osc` and the intercept in `send_event`**

- **Where the intercept goes.** `send_event` matches `UnhandledOsc` and `MouseCursorIcon` before `carries_payload`, and returns from both arms without touching the `TermEvent` channel.
- **When a filter is attached.** Lock it, map the event, and send any `OscEvent` it yields. Wake on every send regardless of `visible`: the sidebar paints progress for hidden sessions and a notification toasts from one.
- **Doc comment.** Say that the filter runs on the PTY thread under the terminal lock, which is why it only classifies. WSL path translation stays in `drain_events`.

- [ ] **Step 5: Replace the tap in `Session::pending` and `open`**

- **`Session::pending`.** Build `OscPolicy` exactly as the `TapPolicy` is built today. If `OscFilter::new` returns `Some`, call `proxy.with_osc(filter)` and store the receiver in `osc_events`.
- **`open`.** Remove `OpenRequest.tap` and the `TeePty` wrap in `open`.
- **Cleanup.** Delete `pty_tee.rs` and its `mod` line; rename `osc_tap` to `osc` in `lib.rs` and at every `use` site.
- **Dependencies and docs.** Restore `polling` to Windows-only; drop `memchr` if unused. Reword the `any_enabled` doc to: a session with none keeps no OSC filter.

- [ ] **Step 6: Convert the tap-driven session tests to feed the terminal**

- **Feeding.** Replace every `request.tap.as_mut().unwrap().offer(bytes)` with `Processor::<StdSyncHandler>::new().advance(&mut *session.term.lock(), bytes)`, then a single `session.drain_events(..)`. Delete the deadline and sleep loops, since delivery is now synchronous.
- **Renames.**
  - `assert_wsl_reported_cwd_through_tap` -> `assert_wsl_reported_cwd`; update its callers at `app.rs:9488,9508`.
  - `disabled_pointer_shape_does_not_create_a_tap` -> `a_session_with_no_vt_key_keeps_no_filter`, asserting `session.osc_events.is_none()`.
- **New case.** In `pointer_shape_osc_bytes_update_the_session_through_the_tap`, now `pointer_shape_osc_updates_the_session`, add `("\x1b]22;hand2\x07", <previous expected>)`: an X11 alias leaves the pointer unchanged.

- [ ] **Step 7: Run everything**

Run: `devkit run task fmt --dir <worktree>` (then restore the unrelated files), `devkit run task test --dir <worktree>`, `devkit run task clippy --dir <worktree>`
Expected: all tests pass, no clippy warnings, `rg -n "pty_tee|osc_tap|TapPolicy|TeePty" <worktree>/alacritree` prints nothing.

- [ ] **Step 8: Commit**

Subject: `refactor(vt): read OSC from the terminal parse instead of a tap`. The body says the tee, the tap thread, its queue and its scanner are gone. OSC events now arrive in stream order under the terminal's own parse, and none are dropped under load.

- [ ] **Step 9: Update PR #383's agent summary**

Fetch the body with `gh pr view 383 --repo alacritree/alacritree --json body -q .body` into the scratchpad.

- **"How it works" paragraph.** Rewrite it for the hook: vendored `vte` hands its uninterpreted OSC sequences to `Term`, which sends them as events; `EventProxy` classifies them and drops repeats before they wake the UI.
- **`polling` line.** Replace it with one noting `vte` is now vendored.
- **Everything else.** Leave the TL;DR and the `Closes` line untouched.

Push the body with `gh pr edit 383 --repo alacritree/alacritree --body-file <file>`, then push the branch with `git -C <worktree> push origin 325-feat-vt-osc-codes`.

## Unresolved questions

1. OSC 22 X11 aliases (`hand2`, `xterm`, `left_ptr`, `watch`, `fleur`) stop working, because `vte` parses OSC 22 itself and accepts only the CSS names from `cursor-icon`. Recommendation: accept that, since it is what upstream's parser accepts. Keeping the aliases costs one more vendored line (call `unhandled_osc` when `CursorIcon::from_str` fails) and a second path for one sequence.
2. Should `UnhandledOsc` carry the cursor's grid line now, for a future OSC 133? Recommendation: no. Add it when OSC 133 lands; `Term` has the cursor at hand, so it is a one-field change to the same vendored arm.
3. Land this on `325-feat-vt-osc-codes` (PR #383), as the plan assumes, or on its own stacked branch? Recommendation: same branch, since #383 is unmerged and this replaces code it introduces.
