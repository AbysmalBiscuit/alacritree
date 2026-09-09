# Palette Row Refinements Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Ctrl+K palette's session rows say the same thing in the same way whether the session is native or herdr-backed, and colour a native agent's status mark by its state rather than by the row's weight.

**Architecture:** All three tasks live in `alacritree/src/app.rs`. Task 1 changes one pure function (`agent_mark`) and its two call sites. Task 2 changes the two palette content builders (`native_palette_content`, `herdr_palette_content`), the shared `session_middle`, and the subtitle's paint style in `paint_palette_row`. Task 3 adds a busy/idle word to a plain shell row's middle column, read from `Session::is_busy`, which is already cached behind the throttled process probe.

**Tech Stack:** Rust 2024, egui/eframe, `cargo nextest` through `devrun task test`.

## Global Constraints

- Work in the worktree `C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette`, on branch `feat/herdr-palette`. Never `cd` to the main checkout.
- Several agents share this checkout. Claim every file before editing it: `DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm acquire <abs path> --note "<why>"`, run from inside the worktree, and release it when the task is done.
- sccache is broken on this machine. Prefix every cargo/devrun invocation with `RUSTC_WRAPPER=` or the compile dies with "Failed to read response header".
- Build, test, lint and format through devkit: `RUSTC_WRAPPER= devrun task test`, `RUSTC_WRAPPER= devrun task clippy`, `RUSTC_WRAPPER= devrun task fmt`. `fmt` runs nightly rustfmt; stable `cargo fmt` reformats files the change never touched.
- Comments explain *why*, never *what*. Timeless and standalone: no `this PR`, `now we`, `used to`, `previously`, no issue or task references, no RED/GREEN narration.
- Conventional Commits, imperative subject under 72 chars, body wrapped at 72. Every commit ends with the trailer `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.
- No new config keys this round. These tasks refine palette rows that exist only on this branch and have never shipped, so there is no prior behaviour for a gate to preserve. A separate follow-focus gate is deliberately deferred to a later branch.
- The middle column spells the agent kind out even when the title already carries it. Consistency across every row beats avoiding one repeated word. This is a deliberate reversal of the current suppression rule and Task 2 removes it.
- Do not touch the sidebar's row text. These changes are palette-only, except Task 1, whose one function is shared with the sidebar's status slot by design.

---

### Task 1: Colour a native agent's status mark by its state

A native agent session's status mark paints its idle glyph in the row's own text colour, so an idle `claude` row shows an uncoloured glyph while a herdr-backed row beside it shows a coloured one. The two go through different arms of `session_status_mark`: herdr rows reach `SessionMark::Harness`, which paints through `theme.harness_state.of(tone)`, and native rows reach `SessionMark::Agent`, which paints through a `quiet` colour the caller derives from whether the row is active. Route the native arm through the same `StateColors` the harness arm uses.

**Files:**
- Modify: `alacritree/src/app.rs` (`agent_mark` and its two call sites)
- Test: `alacritree/src/app.rs` (in-module `#[cfg(test)]`, the existing `agent_mark` tests near line 13560)

**Interfaces:**
- Consumes: `StateTone` (`app.rs:8083`), `StateColors::of` (`app.rs:132`), `Theme::harness_state` (`app.rs:94`), `LiveState` (`session.rs:145`).
- Produces: `fn agent_mark(live: LiveState, theme: &Theme) -> AgentMark`. Both call sites lose their `quiet`/`attention` arguments.

- [ ] **Step 1: Write the failing test**

Replace the existing `agent_mark` colour assertions with one that pins the state-tone mapping. Add this test beside them:

```rust
    /// A native agent's mark is coloured by the state it reports, the same
    /// way a harness-backed row's mark is, so two rows in one state never
    /// disagree about what that state looks like.
    #[test]
    fn an_agent_mark_takes_its_colour_from_the_state_it_reports() {
        let theme = Theme::from_config(&Config::default());
        assert_eq!(
            agent_mark(LiveState::Idle, &theme),
            AgentMark::Glyph(DEFAULT_AGENT_ICON, theme.harness_state.of(StateTone::Idle))
        );
        assert_eq!(
            agent_mark(LiveState::Blocked, &theme),
            AgentMark::Glyph(DEFAULT_BLOCKED_ICON, theme.harness_state.of(StateTone::Blocked))
        );
        assert_eq!(agent_mark(LiveState::Working, &theme), AgentMark::Loader);
    }
```

- [ ] **Step 2: Run it and watch it fail**

```sh
RUSTC_WRAPPER= cargo nextest run -p alacritree --locked -E 'test(agent_mark)'
```

Expected: a compile error, because `agent_mark` still takes three arguments.

- [ ] **Step 3: Change the function**

```rust
/// Colour comes from the state, not from the row: an idle agent reads the
/// same on a selected row as on a quiet one, and the same as a harness-backed
/// row reporting the same state.  Working animates because a static glyph
/// would have to blink to say as much.
fn agent_mark(live: LiveState, theme: &Theme) -> AgentMark {
    match live {
        LiveState::Idle => {
            AgentMark::Glyph(DEFAULT_AGENT_ICON, theme.harness_state.of(StateTone::Idle))
        },
        LiveState::Working => AgentMark::Loader,
        LiveState::Blocked => {
            AgentMark::Glyph(DEFAULT_BLOCKED_ICON, theme.harness_state.of(StateTone::Blocked))
        },
    }
}
```

- [ ] **Step 4: Update both call sites**

In `paint_palette_row` (near `app.rs:6517`), replace:

```rust
            SessionMark::Agent(live) => {
                let quiet = if selected { theme.accent } else { theme.text };
                paint_agent_mark(ui, agent_mark(live, quiet, theme.attention), mark_rect, theme);
            },
```

with:

```rust
            SessionMark::Agent(live) => {
                paint_agent_mark(ui, agent_mark(live, theme), mark_rect, theme)
            },
```

In `paint_row_status_icon` (near `app.rs:7174`), replace:

```rust
        Some((SessionMark::Agent(live), hint)) => {
            let (rect, _) =
                ui.allocate_exact_size(row_status_icon_size(theme), egui::Sense::hover());
            let quiet = if is_active { theme.accent } else { theme.text };
            paint_agent_mark(ui, agent_mark(live, quiet, theme.attention), rect, theme);
            Some((rect, hint))
        },
```

with:

```rust
        Some((SessionMark::Agent(live), hint)) => {
            let (rect, _) =
                ui.allocate_exact_size(row_status_icon_size(theme), egui::Sense::hover());
            paint_agent_mark(ui, agent_mark(live, theme), rect, theme);
            Some((rect, hint))
        },
```

If `is_active` is left unused in `paint_row_status_icon` after this, keep the parameter only if another arm still reads it. The `None` arm does read it, so it stays.

- [ ] **Step 5: Repair the old tests**

The existing tests near `app.rs:13560` assert the old three-argument shape, including one that pins "Blocked takes attention over an active row's accent". Delete the assertions that no longer describe the function and keep any that still hold. Do not keep a test whose only content is that a function compiles.

- [ ] **Step 6: Run the suite**

```sh
RUSTC_WRAPPER= devrun task test
```

Expected: every test passes.

- [ ] **Step 7: Format, lint, commit**

```sh
RUSTC_WRAPPER= devrun task fmt
RUSTC_WRAPPER= devrun task clippy
git add alacritree/src/app.rs
git commit
```

Subject: `fix(palette): colour a native agent's mark by its state`

---

### Task 2: One column contract for native and herdr rows

Native and herdr session rows put different things in the same columns. The middle column drops the agent kind whenever the title already contains it, so two rows in the same state read differently. A herdr row says nothing about herdr in its middle column. The left column's second line is smaller and greyer than the line above it, which buries the workspace it names.

The contract this task establishes:

- Middle column, native: `<kind> · <state>`, kind always spelled out.
- Middle column, herdr: `herdr · <kind> · <state>`, in that order.
- Left column line 1: the session title.
- Left column line 2: the workspace, at `font_normal - 1.0` in `theme.text`, led by the herdr glyph on a herdr row.
- A generic title keeps its current native behaviour: the workspace is promoted to line 1 and line 2 goes empty. A herdr row does the same, keeping the glyph alone on line 2 so the row still reads as herdr-backed.

**Files:**
- Modify: `alacritree/src/app.rs` (`session_middle`, `native_palette_content`, `herdr_palette_content`, `herdr_subtitle`, `paint_palette_row`)
- Test: `alacritree/src/app.rs` (in-module `#[cfg(test)]`)

**Interfaces:**
- Consumes: `PaletteSessionContent` (`app.rs:7845`), `herdr::Agent` (`herdr.rs:85`), `Theme::text` and `Theme::font_normal`.
- Produces: `session_middle` gains a leading-token parameter so a herdr row can prefix `herdr`. Signature: `fn session_middle(lead: Option<&str>, kind: Option<&str>, status: Option<&str>, fallback: &str) -> String`. The `title` parameter goes away with the suppression rule that used it.

- [ ] **Step 1: Write the failing tests**

```rust
    /// The kind is spelled out even when the title already carries it: two
    /// rows in one state must read the same, and one repeated word is a
    /// cheaper price than a column that changes shape per row.
    #[test]
    fn the_middle_column_spells_the_kind_out_beside_a_title_that_shares_it() {
        assert_eq!(session_middle(None, Some("claude"), Some("idle"), "shell"), "claude · idle");
    }

    /// A herdr-backed row names herdr ahead of the agent, so the palette says
    /// where a row comes from without the reader decoding a glyph.
    #[test]
    fn a_herdr_rows_middle_column_leads_with_herdr() {
        assert_eq!(
            session_middle(Some("herdr"), Some("codex"), Some("working"), "shell"),
            "herdr · codex · working"
        );
    }

    /// A lead with nothing after it still names itself rather than falling
    /// through to the fallback: the row is herdr-backed whatever else is
    /// unknown about it.
    #[test]
    fn a_lead_survives_an_otherwise_empty_middle_column() {
        assert_eq!(session_middle(Some("herdr"), None, None, "shell"), "herdr · shell");
    }
```

- [ ] **Step 2: Run them and watch them fail**

```sh
RUSTC_WRAPPER= cargo nextest run -p alacritree --locked -E 'test(middle_column) or test(session_middle)'
```

Expected: a compile error on the new `session_middle` arity.

- [ ] **Step 3: Rewrite `session_middle`**

```rust
/// The middle column's words, most general first: where the row comes from,
/// what runs in it, what that is doing.  The kind is spelled out whether or
/// not the title repeats it, so every row in one state reads identically.
fn session_middle(
    lead: Option<&str>,
    kind: Option<&str>,
    status: Option<&str>,
    fallback: &str,
) -> String {
    let mut parts: Vec<String> = lead.map(str::to_owned).into_iter().collect();
    parts.extend(kind.map(str::to_lowercase));
    parts.extend(status.map(str::to_owned));
    if parts.len() == lead.iter().count() {
        parts.push(fallback.to_string());
    }
    parts.retain(|part| !part.is_empty());
    parts.join(" · ")
}
```

- [ ] **Step 4: Update `native_palette_content`**

It keeps its generic-title behaviour exactly as it stands. Only the `session_middle` call changes, losing the title argument and gaining a `None` lead:

```rust
        secondary: session_middle(None, kind, status, fallback),
```

Delete `contains_case_insensitive` if nothing else calls it.

- [ ] **Step 5: Update `herdr_palette_content`**

Its `session_middle` call gains the `herdr` lead and loses the title:

```rust
        secondary: session_middle(
            Some("herdr"),
            agent.kind.as_deref(),
            agent.status.map(|status| status.label()),
            fallback,
        ),
```

In the generic branch, an unattached row currently puts `agent.pane_id` on line 2 while an attached one puts the glyph. Give both the glyph, so a generic herdr row reads the same attached or not:

```rust
    let subtitle = if generic {
        glyph.to_string()
    } else {
        let location = workspace.or(abbreviated_cwd.as_deref());
        herdr_subtitle(glyph, location)
    };
```

- [ ] **Step 6: Restyle the subtitle**

In `paint_palette_row`, the subtitle is built at `font_normal - 2.0` in `theme.text_muted` and painted in `theme.text_muted`. Raise it and un-grey it. Replace:

```rust
    let subtitle_size = (theme.font_normal - 2.0).max(8.0);
```

with:

```rust
    let subtitle_size = (theme.font_normal - 1.0).max(8.0);
```

and both `theme.text_muted` arguments in the subtitle's `column_galley` call and its `painter.galley` call with `theme.text`.

- [ ] **Step 7: Fix the rest of the call sites and tests**

`rg -n "session_middle\(" alacritree/src/app.rs` finds every caller. The `!current` branch in `palette_items` (near `app.rs:10379`) also calls it and needs the `herdr` lead, since that row is still a herdr row whose status has gone stale.

Existing tests near `app.rs:12060-12250` and `app.rs:13980-14180` assert row content. Update the ones the new contract changes. A test asserting the old suppression rule is describing behaviour this task removes: delete it rather than inverting it in place.

- [ ] **Step 8: Run the suite**

```sh
RUSTC_WRAPPER= devrun task test
```

- [ ] **Step 9: Format, lint, commit**

```sh
RUSTC_WRAPPER= devrun task fmt
RUSTC_WRAPPER= devrun task clippy
git add alacritree/src/app.rs
git commit
```

Subject: `feat(palette): give native and herdr rows one column contract`

---

### Task 3: A plain shell row says whether it is busy

A shell row's middle column is the bare word `shell`, which says nothing a reader could act on. `Session::is_busy` already answers busy-or-idle from the throttled process probe, so the word costs one call and no new plumbing.

Scope: native sessions of kind `SessionKind::Shell` only.

- A herdr row keeps the bare `shell`. herdr reports no status for a pane it found no agent in, and an attached herdr session's own PTY runs the attach client, so `is_busy` would answer "busy" forever and mean nothing.
- `SessionKind::Diff` and `SessionKind::Scratchpad` keep their bare kind word. Neither has a foreground job worth reporting.

**Files:**
- Modify: `alacritree/src/app.rs` (the native branch of `palette_items`)
- Test: `alacritree/src/app.rs` (in-module `#[cfg(test)]`)

**Interfaces:**
- Consumes: `Session::is_busy` (`session.rs:1686`), `SessionKind` (`session.rs:125`), `session_fallback_kind` (`app.rs:7992`), `session_middle` as Task 2 leaves it.
- Produces: `fn shell_state(session: &Session) -> Option<&'static str>` returning `Some("busy")`, `Some("idle")`, or `None` for a kind with no foreground job to report.

- [ ] **Step 1: Write the failing test**

```rust
    /// A shell row reports whether a job holds the terminal, which is the one
    /// thing about a shell worth reading off a list.  A kind with no
    /// foreground job of its own reports nothing rather than a state it
    /// cannot observe.
    #[test]
    fn only_a_plain_shell_reports_a_busy_state() {
        assert_eq!(shell_state_for(&SessionKind::Shell, true), Some("busy"));
        assert_eq!(shell_state_for(&SessionKind::Shell, false), Some("idle"));
        assert_eq!(shell_state_for(&SessionKind::Scratchpad { path: PathBuf::new() }, true), None);
        assert_eq!(shell_state_for(&SessionKind::Diff { key: "k".into() }, true), None);
    }
```

- [ ] **Step 2: Run it and watch it fail**

```sh
RUSTC_WRAPPER= cargo nextest run -p alacritree --locked -E 'test(busy_state)'
```

Expected: `shell_state_for` is not defined.

- [ ] **Step 3: Add the helper**

Put it beside `session_fallback_kind`. Split so the decision is testable without a live `Session`:

```rust
/// Whether a shell row can say what it is doing.  Only a plain shell has a
/// foreground job to read; a diff or scratchpad row has no process behind it
/// and would be inventing a state.
fn shell_state_for(kind: &SessionKind, busy: bool) -> Option<&'static str> {
    match kind {
        SessionKind::Shell => Some(if busy { "busy" } else { "idle" }),
        SessionKind::Diff { .. } | SessionKind::Scratchpad { .. } => None,
    }
}
```

- [ ] **Step 4: Call it from the native palette branch**

In `palette_items`, the native arm currently reads:

```rust
            let (agent_kind, status) = match activity {
                SessionActivity::Agent { name, live } => (name, Some(live.label())),
                SessionActivity::Shell => (None, None),
            };
```

Give the shell arm the session's kind word and its state, so `session_middle` receives a kind rather than falling through to its fallback:

```rust
            let fallback_kind = session_fallback_kind(&session.kind);
            let (agent_kind, status) = match activity {
                SessionActivity::Agent { name, live } => (name, Some(live.label())),
                SessionActivity::Shell => {
                    (Some(fallback_kind), shell_state_for(&session.kind, session.is_busy()))
                },
            };
```

`agent_kind` is passed on to `PaletteItem::session` as the search haystack's agent term and to `palette_hover` as the hover's `Kind:` line. Both already accept the kind word for a non-agent session, so no further change is needed there. Confirm that by reading both call sites rather than assuming it.

- [ ] **Step 5: Run the suite**

```sh
RUSTC_WRAPPER= devrun task test
```

- [ ] **Step 6: Format, lint, commit**

```sh
RUSTC_WRAPPER= devrun task fmt
RUSTC_WRAPPER= devrun task clippy
git add alacritree/src/app.rs
git commit
```

Subject: `feat(palette): say whether a shell row is busy`
