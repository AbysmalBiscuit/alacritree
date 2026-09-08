# Herdr in the palette and the sidebar filter — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** reach a herdr agent by typing its name — from the command palette, where an attached agent is focused and an unattached one is attached, and from the sidebar filter, where both appear as rows instead of vanishing.

**Architecture:** one shared agent listing (a free function over slices) feeds both surfaces, so `show_unmatched` cannot be honoured by one and ignored by the other. The sidebar half moves three pieces that must agree — what `sidebar_nav::filtered_rows` emits, what the painter draws, and what `sidebar_focus` watches for change — and the task order below keeps every intermediate state behaviour-identical so no commit ships a half-moved sidebar.

**Tech Stack:** Rust 2024, egui/eframe, `nucleo` fuzzy matching (already wired through `PanelFilter` and `CommandPalette`), `cargo nextest` via devkit.

**Spec:** `docs/superpowers/specs/2026-09-07-herdr-palette-and-filter-design.md`

## Global Constraints

- Branch `feat/herdr-palette`, marker `[15]`, stacked on `feat/config-schema-defaults` (`[14]`), itself stacked on PR 215. Re-point the branch after `devkit issue setup 77 --slug feat/herdr-palette`, before it has commits: `git -C <worktree> reset --hard origin/feat/config-schema-defaults`. Read the stack tip fresh — `gh pr list --repo mathix420/alacritree --state open --json number,title,headRefName`.
- No new config keys. The herdr rows are already gated by `[integrations.herdr] enabled`; the sidebar-filter half is gated by a non-empty query, established in Task 8.
- All work in `alacritree/`. The vendored crates (`alacritty/`, `alacritty_terminal/`, `alacritty_config*/`, `egui-winit/`) are read-only.
- Build and test through devkit only: `devkit run task check`, `devkit run task test`, `devkit run task clippy`, `devkit run task fmt`. Bare `cargo fmt`, `cargo test` and `cargo clippy` are refused by `[harness] enforce_commands`.
- Claim every file before editing it: `DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm acquire <abs path> --note "<why>"`. Several agents share this checkout.
- Commit trailer on every commit: `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.
- Conventional Commits, imperative subject, ≤72 chars.
- Comments explain *why*, never restate *what*, and are one line unless a second carries a fact the first does not. No PR/task references, no change-relative phrasing.
- `sidebar_nav.rs` and `command_palette.rs` stay egui-free apart from the key/modifier types `command_palette.rs` already imports — that is what makes them unit-testable.
- The reconciler's unchanged-frame path must stay allocation-free; `steady_state.rs` asserts it. `ObservedInputs::matches` may not allocate. `ObservedInputs::capture` may.
- `devkit run task` takes no passthrough arguments, so there is no `devkit run task test -- <filter>`. To run one test, append the filter to the task's own argv: `cargo nextest run -p alacritree --locked <filter>` (`devkit run task test --dry-run` prints that argv).

## File structure

| File | Responsibility | Tasks |
| --- | --- | --- |
| `alacritree/src/app.rs` | `listed_herdr_agents` free fn, its `&self` wrapper, palette row construction, `run_palette_action` dispatch, `PendingHerdrAttach`, the sidebar painter, `current_project_rows`, `session_inputs` | 1–8 |
| `alacritree/src/herdr.rs` | `Side::label` | 2 |
| `alacritree/src/command_palette.rs` | `HerdrAttach`, `PaletteAction::AttachHerdrAgent`, `PaletteSection::HerdrAgents`, `PaletteItem::herdr_agent` | 3 |
| `alacritree/src/sidebar_nav.rs` | `RowPredicates` shape, `filtered_rows` emission rules | 6, 8 |
| `alacritree/src/sidebar_focus.rs` | `SessionInput.title`, `ObservedInputs` capture/compare | 7 |
| `docs/alacritree.md` | user-facing description of both surfaces | 9 |

`app.rs` is already large. This plan adds one free function and one wrapper to it rather than splitting it: the new logic is small, and the pure pieces worth isolating (`filtered_rows`, `PaletteItem`) already live in their own modules.

---

### Task 1: One agent listing, two surfaces

Extract the agent walk out of `listed_workspace_rows` into a free function over slices, so the palette reads the same listing the sidebar does and `show_unmatched` cannot be honoured by one and forgotten by the other. A free function rather than a method because there is no `AlacritreeApp` test fixture — `Session::new` opens a PTY and `EndpointCache::agents` has no constructor but `poll`.

**Files:**
- Modify: `alacritree/src/app.rs` (add free fn near `herdr_workspaces` at `:8367`; rewrite the agent block of `listed_workspace_rows` at `:8886-8908`)
- Test: `alacritree/src/app.rs` `#[cfg(test)] mod tests`, beside `herdr_agent` at `:11933`

**Interfaces:**
- Consumes: `herdr::unattached` (`herdr.rs:318`), `herdr::match_workspace` (`herdr.rs:341`), `herdr::EndpointCache::{side, agents}` (`herdr.rs:482`, `:486`), `herdr_workspaces` (`app.rs:8367`)
- Produces:
  - `fn listed_herdr_agents<'a>(caches: &'a [herdr::EndpointCache], claimed: &[herdr::HerdrKey], workspaces: &[PathBuf], show_unmatched: bool) -> Vec<(WorkspaceKey, &'a herdr::Side, &'a herdr::Agent)>`
  - `fn AlacritreeApp::herdr_agent_listing(&self) -> Vec<(WorkspaceKey, &herdr::Side, &herdr::Agent)>`

- [ ] **Step 1: Claim the file**

```bash
DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm acquire \
  "$(pwd)/alacritree/src/app.rs" --note "herdr palette: shared agent listing"
```

- [ ] **Step 2: Write the failing tests**

Add to `app.rs`'s existing `mod tests`, after the `herdr_agent` helper at `:11933`. `herdr_agent` builds an agent with `cwd: None`, so `match_workspace` returns `None` for it and it lands under Home — which is exactly the `show_unmatched` case under test.

```rust
/// An agent with a working directory, for the bucketing cases.
fn agent_in(dir: &str) -> herdr::Agent {
    herdr::Agent { cwd: Some(dir.into()), ..herdr_agent(Some("claude")) }
}

fn cache_with(agents: Vec<herdr::Agent>) -> herdr::EndpointCache {
    let mut cache = herdr::EndpointCache::new(herdr::Side::Native);
    cache.set_agents_for_test(agents);
    cache
}

/// `show_unmatched` is the sidebar's setting, and the palette reads the same
/// listing, so an agent it hides is hidden from both by construction.
#[test]
fn an_unmatched_agent_is_absent_when_show_unmatched_is_off() {
    let caches = [cache_with(vec![herdr_agent(Some("claude"))])];
    assert!(listed_herdr_agents(&caches, &[], &[], false).is_empty());
    assert_eq!(listed_herdr_agents(&caches, &[], &[], true).len(), 1);
}

/// An agent an open session holds is not listed: its row is that session's.
#[test]
fn a_claimed_agent_is_absent_from_the_listing() {
    let agent = herdr_agent(Some("claude"));
    let claimed = [herdr::HerdrKey {
        side: herdr::Side::Native,
        terminal_id: agent.terminal_id.clone(),
    }];
    let caches = [cache_with(vec![agent])];
    assert!(listed_herdr_agents(&caches, &claimed, &[], true).is_empty());
}

/// The workspace an agent is bucketed under is the longest matching worktree
/// path, which is what the palette carries in its attach payload.
#[test]
fn a_matched_agent_carries_its_workspace() {
    let dir = if cfg!(windows) { r"C:\p\wt" } else { "/p/wt" };
    let caches = [cache_with(vec![agent_in(dir)])];
    let workspaces = vec![PathBuf::from(dir)];
    let listed = listed_herdr_agents(&caches, &[], &workspaces, true);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].0, Some(PathBuf::from(dir)));
}
```

- [ ] **Step 3: Run the tests to verify they fail**

```bash
cargo nextest run -p alacritree --locked listed_herdr_agents
```

Expected: FAIL to compile — `cannot find function 'listed_herdr_agents'`, and `no method named 'set_agents_for_test'`.

- [ ] **Step 4: Add the test-only cache constructor**

In `herdr.rs`, inside `impl EndpointCache` after `agents()` at `:486`:

```rust
#[cfg(test)]
pub fn set_agents_for_test(&mut self, agents: Vec<Agent>) {
    self.agents = agents;
}
```

- [ ] **Step 5: Write the free function**

In `app.rs`, immediately after `herdr_workspaces` (`:8374`):

```rust
/// Every herdr agent no session holds, with the workspace it belongs under.
/// The sidebar and the palette both read this, so an agent hidden from one is
/// hidden from the other by construction rather than by two filters agreeing.
fn listed_herdr_agents<'a>(
    caches: &'a [herdr::EndpointCache],
    claimed: &[herdr::HerdrKey],
    workspaces: &[PathBuf],
    show_unmatched: bool,
) -> Vec<(WorkspaceKey, &'a herdr::Side, &'a herdr::Agent)> {
    let mut listed = Vec::new();
    for cache in caches {
        let side = cache.side();
        for agent in herdr::unattached(cache.agents(), side, claimed) {
            let ws = herdr::match_workspace(agent, side, workspaces);
            if ws.is_none() && !show_unmatched {
                continue;
            }
            listed.push((ws, side, agent));
        }
    }
    listed
}
```

- [ ] **Step 6: Run the tests to verify they pass**

```bash
cargo nextest run -p alacritree --locked listed_herdr_agents
```

Expected: PASS, 3 tests.

- [ ] **Step 7: Add the `&self` wrapper**

In `app.rs`, immediately before `listed_workspace_rows` (`:8867`):

```rust
/// `listed_herdr_agents` against this frame's own state.  Empty while herdr
/// is disabled, so a caller never has to ask twice.
fn herdr_agent_listing(&self) -> Vec<(WorkspaceKey, &herdr::Side, &herdr::Agent)> {
    if !self.config.integrations.herdr.enabled {
        return Vec::new();
    }
    let claimed: Vec<herdr::HerdrKey> =
        self.sessions.iter().filter_map(|s| s.herdr_key.clone()).collect();
    let workspaces = herdr_workspaces(&self.projects, |path| self.liveness.missing(path));
    listed_herdr_agents(
        self.herdr_endpoints.caches(),
        &claimed,
        &workspaces,
        self.config.integrations.herdr.show_unmatched,
    )
}
```

- [ ] **Step 8: Rewrite the agent block of `listed_workspace_rows`**

Replace the whole `if self.config.integrations.herdr.enabled { ... }` block at `app.rs:8886-8908` with:

```rust
for (ws, side, agent) in self.herdr_agent_listing() {
    let key =
        herdr::HerdrKey { side: side.clone(), terminal_id: agent.terminal_id.clone() };
    let at = self.herdr_pane_index(&key).unwrap_or(usize::MAX);
    let entry = WorkspaceEntry::Agent(side.clone(), agent.terminal_id.clone());
    managed.entry(ws).or_default().push((at, entry));
}
```

- [ ] **Step 9: Run the full suite**

```bash
devkit run task test
devkit run task clippy
```

Expected: PASS. The existing herdr sidebar tests are the regression net — the listing they see must be identical.

- [ ] **Step 10: Format and commit**

```bash
devkit run task fmt
git add alacritree/src/app.rs alacritree/src/herdr.rs
git commit -m "refactor(herdr): share the agent listing between surfaces

The palette needs the same unattached-agent walk the sidebar does, and
two copies would let show_unmatched be honoured by one and forgotten by
the other. Extracting it over slices also makes it testable: there is no
AlacritreeApp fixture, and EndpointCache fills only through poll.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 2: Attached agents get the sidebar's name in the palette

A herdr-attached session is already an `OpenSessions` row, but its `primary` is `session.title` — the PTY title, which for a shared-view attach names herdr's whole window rather than the agent. The sidebar solved this with `session_row_name`; the palette reuses it.

Use `session_row_name` and **not** `HerdrRowData::from_agent`: that chain is for an *un*attached agent with no PTY of its own, so it falls back to the kind and then the terminal id's tail. An attached session has a PTY, and `session_row_name` falls back to its title with the kind as context.

**Files:**
- Modify: `alacritree/src/herdr.rs` (add `Side::label` inside `impl Side` at `:165`)
- Modify: `alacritree/src/app.rs:9528-9534` (the session loop in `palette_items`)
- Test: `alacritree/src/app.rs` `mod tests`

**Interfaces:**
- Consumes: `session_row_name` (`app.rs:8003`), `herdr_backed_activity` + `session_herdr_status` (`app.rs:9050`), `session_herdr_agent` (`app.rs:8972`), `herdr::Status::label` (`herdr.rs:69`), `RowName { text, context }` (`app.rs:7367`)
- Produces:
  - `fn herdr::Side::label(&self) -> Option<String>`
  - `fn AlacritreeApp::herdr_palette_secondary(&self, side: &herdr::Side, status: herdr::Status, context: Option<&str>, workspace: &WorkspaceKey) -> String`

- [ ] **Step 1: Write the failing test**

In `app.rs`'s `mod tests`:

```rust
/// The native side would name itself the same word on every row of a machine
/// that has only it, so only a WSL endpoint is worth spelling out.
#[test]
fn only_a_wsl_side_is_labelled() {
    assert_eq!(herdr::Side::Native.label(), None);
    assert_eq!(herdr::Side::Wsl("Ubuntu".into()).label(), Some("wsl:Ubuntu".into()));
}
```

- [ ] **Step 2: Run it to verify it fails**

```bash
cargo nextest run -p alacritree --locked only_a_wsl_side_is_labelled
```

Expected: FAIL to compile — `no method named 'label' found for enum 'Side'`.

- [ ] **Step 3: Add `Side::label`**

In `herdr.rs`, inside `impl Side` after `command` (`:183`):

```rust
/// How a row names this side.  `None` on the native one, whose name would
/// be the same word on every row of a machine that has only it.
pub fn label(&self) -> Option<String> {
    match self {
        Self::Native => None,
        Self::Wsl(distro) => Some(format!("wsl:{distro}")),
    }
}
```

- [ ] **Step 4: Run it to verify it passes**

```bash
cargo nextest run -p alacritree --locked only_a_wsl_side_is_labelled
```

Expected: PASS.

- [ ] **Step 5: Add the shared secondary-column builder**

In `app.rs`, immediately after `workspace_label` (`:9570`):

```rust
/// The secondary column a herdr row shows: the integration, then whatever
/// context the name did not already carry, then herdr's own status word,
/// then where the row lives.  Shared by the attached and unattached rows so
/// one heading's worth of vocabulary reads the same across both.
fn herdr_palette_secondary(
    &self,
    side: &herdr::Side,
    status: herdr::Status,
    context: Option<&str>,
    workspace: &WorkspaceKey,
) -> String {
    let mut parts = vec!["herdr".to_string()];
    parts.extend(context.map(str::to_string));
    parts.push(status.label().to_string());
    parts.extend(side.label());
    parts.push(self.workspace_label(workspace));
    parts.join(" · ")
}
```

- [ ] **Step 6: Rewrite the session loop in `palette_items`**

Replace `app.rs:9528-9534` with:

```rust
for session in &self.sessions {
    let agent = self.session_herdr_agent(session);
    let activity = herdr_backed_activity(session.activity(), self.session_herdr_status(session));
    let name = session_row_name(&session.title, activity, agent);
    let secondary = match agent {
        Some(a) => self.herdr_palette_secondary(
            &session.herdr_key.as_ref().expect("an agent implies a key").side,
            a.status,
            name.context.as_deref(),
            &session.working_directory,
        ),
        None => format!("session · {}", self.workspace_label(&session.working_directory)),
    };
    items.push(PaletteItem::session(session.id, name.text, secondary));
}
```

- [ ] **Step 7: Build and run the full suite**

```bash
devkit run task check
devkit run task test
```

Expected: PASS. No test asserts on the old palette session label, so nothing should break.

- [ ] **Step 8: Commit**

```bash
devkit run task fmt
git add alacritree/src/app.rs alacritree/src/herdr.rs
git commit -m "feat(palette): name attached herdr rows the way the sidebar does

A shared-view attach titles its PTY after herdr's whole window, so the
palette row named no agent at all. session_row_name already prefers what
herdr reports, and the secondary column now says which agent and what it
is doing.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 3: Unattached agents get a palette section

One new section, `Herdr agents`, built right after the session block so an empty query orders actions, profiles, sessions, herdr agents, workspaces, worktrees. Attached agents stay under `Open sessions`, because an attached agent *is* an open session and Enter focuses it exactly as it focuses any session — splitting them would mean knowing which heading to look under before looking.

`PaletteItem::search` is private and built in `new` from `primary`/`secondary`/`keys`, so the unpainted `herdr` haystack entry needs a constructor beside `session`, following `profile` (`command_palette.rs:168`).

**Files:**
- Modify: `alacritree/src/command_palette.rs:24-38` (`PaletteAction`), `:42-70` (`PaletteSection`), `:118+` (`impl PaletteItem`)
- Modify: `alacritree/src/app.rs` (`palette_items` at `:9515`, `run_palette_action` at `:9580`)
- Test: `alacritree/src/command_palette.rs` `mod tests` at `:383`

**Interfaces:**
- Consumes: `AlacritreeApp::herdr_agent_listing` (Task 1), `AlacritreeApp::herdr_palette_secondary` (Task 2), `attach_herdr_agent` (`app.rs:1419`), `herdr_row_name` (`app.rs:7382`)
- Produces:
  - `pub struct HerdrAttach { pub key: herdr::HerdrKey, pub pane_id: String, pub workspace: WorkspaceKey }` with `#[derive(Debug, Clone, PartialEq)]`
  - `PaletteAction::AttachHerdrAgent(HerdrAttach)`
  - `PaletteSection::HerdrAgents`, titled `"Herdr agents"`
  - `pub fn PaletteItem::herdr_agent(attach: HerdrAttach, primary: String, secondary: String) -> Self`

- [ ] **Step 1: Claim the file**

```bash
DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm acquire \
  "$(pwd)/alacritree/src/command_palette.rs" --note "herdr palette: agent rows"
```

- [ ] **Step 2: Write the failing tests**

In `command_palette.rs`'s `mod tests`:

```rust
fn attach(id: &str) -> HerdrAttach {
    HerdrAttach {
        key: crate::herdr::HerdrKey {
            side: crate::herdr::Side::Native,
            terminal_id: id.into(),
        },
        pane_id: "w5:p1".into(),
        workspace: None,
    }
}

/// One heading is enough only if the integration's own name reaches both
/// kinds of row, so `herdr` is folded into the haystack of the unattached
/// ones and already sits in the attached ones' secondary column.
#[test]
fn typing_herdr_ranks_attached_and_unattached_rows() {
    let items = vec![
        PaletteItem::session(1, "fix the wrap bug".into(), "herdr · claude · working · Home".into()),
        PaletteItem::herdr_agent(attach("t1"), "review the schema".into(), "claude · idle · Home".into()),
        PaletteItem::session(2, "nvim config".into(), "session · Home".into()),
    ];
    let mut palette = CommandPalette::new();
    palette.query_mut().push_str("herdr");
    let ranked = palette.rank(&items);
    assert!(ranked.contains(&0), "the attached row is missing");
    assert!(ranked.contains(&1), "the unattached row is missing");
    assert!(!ranked.contains(&2), "a plain session row should not match");
}

/// Grouping is by first appearance, so building the agent block right after
/// the session block is what puts the heading under Open sessions.
#[test]
fn herdr_agents_group_after_open_sessions() {
    let items = vec![
        PaletteItem::session(1, "nvim".into(), "session · Home".into()),
        PaletteItem::herdr_agent(attach("t1"), "review".into(), "claude · idle · Home".into()),
    ];
    let mut palette = CommandPalette::new();
    let ranked = palette.rank(&items);
    let sections: Vec<PaletteSection> =
        group(&items, &ranked).into_iter().map(|(s, _)| s).collect();
    assert_eq!(sections, vec![PaletteSection::OpenSessions, PaletteSection::HerdrAgents]);
}
```

- [ ] **Step 3: Run them to verify they fail**

```bash
cargo nextest run -p alacritree --locked herdr
```

Expected: FAIL to compile — `cannot find struct 'HerdrAttach'`, `no function 'herdr_agent'`, `no variant 'HerdrAgents'`.

- [ ] **Step 4: Add the action, the payload and the section**

In `command_palette.rs`, after the `use` block at `:20`:

```rust
/// A herdr agent no session holds, and where attaching to it lands.  The
/// workspace rides along because the listing already resolved it: looking it
/// up again would go through `herdr_row_workspace`, whose outer `None` means
/// "listed nowhere" — a state the palette can reach and the sidebar cannot.
#[derive(Debug, Clone, PartialEq)]
pub struct HerdrAttach {
    pub key: crate::herdr::HerdrKey,
    pub pane_id: String,
    pub workspace: WorkspaceKey,
}
```

Add to `PaletteAction` (`:25`), after `SpawnProfile`:

```rust
/// Attach to a herdr agent no session holds, in the workspace its working
/// directory matched.
AttachHerdrAgent(HerdrAttach),
```

Add to `PaletteSection` (`:42`), after `OpenSessions`:

```rust
HerdrAgents,
```

and to the exhaustive `title` match (`:56`), after the `OpenSessions` arm:

```rust
Self::HerdrAgents => "Herdr agents",
```

- [ ] **Step 5: Add the constructor**

In `impl PaletteItem`, after `profile` (`:181`):

```rust
/// A herdr agent nothing is attached to.  `herdr` is folded into the
/// search haystack without being painted, so the integration's own name
/// finds these rows as well as the attached ones, whose secondary column
/// already carries it.
pub fn herdr_agent(attach: HerdrAttach, primary: String, secondary: String) -> Self {
    let mut item = Self::new(
        PaletteAction::AttachHerdrAgent(attach),
        PaletteSection::HerdrAgents,
        String::new(),
        primary,
        secondary,
    );
    item.search.push_str(" herdr");
    item
}
```

- [ ] **Step 6: Run the tests to verify they pass**

```bash
cargo nextest run -p alacritree --locked herdr
```

Expected: PASS, 2 tests.

- [ ] **Step 7: Build the rows in `palette_items`**

In `app.rs`, immediately after the session loop and before the `workspace_order` loop (`:9535`):

```rust
for (ws, side, agent) in self.herdr_agent_listing() {
    // A listed row has nothing better than the terminal id's tail behind
    // the kind, so the kind takes the name rather than standing in front
    // of six characters nobody reads.
    let name = herdr_row_name(agent).unwrap_or_else(|| {
        RowName::plain(agent.kind.clone().unwrap_or_else(|| {
            let id = &agent.terminal_id;
            let skip = id.chars().count().saturating_sub(6);
            id.chars().skip(skip).collect()
        }))
    });
    let secondary =
        self.herdr_palette_secondary(side, agent.status, name.context.as_deref(), &ws);
    items.push(PaletteItem::herdr_agent(
        command_palette::HerdrAttach {
            key: herdr::HerdrKey {
                side: side.clone(),
                terminal_id: agent.terminal_id.clone(),
            },
            pane_id: agent.pane_id.clone(),
            workspace: ws.clone(),
        },
        name.text,
        secondary,
    ));
}
```

- [ ] **Step 8: Dispatch Enter**

In `run_palette_action` (`app.rs:9580`), after the `SpawnProfile` arm:

```rust
PaletteAction::AttachHerdrAgent(a) => {
    // Switches first, same as both sidebar paths: a refusal is only
    // visible if the workspace it happened in is on screen.
    let previous = std::mem::replace(&mut self.current_workspace, a.workspace.clone());
    if self.attach_herdr_agent(ctx, a.key, &a.pane_id, a.workspace) {
        self.focus_terminal();
    } else {
        self.current_workspace = previous;
    }
},
```

- [ ] **Step 9: Run the full suite and check clippy**

```bash
devkit run task check
devkit run task test
devkit run task clippy
```

Expected: PASS.

- [ ] **Step 10: Smoke-test by hand**

Start a herdr server with at least one agent, run `cargo run -p alacritree`, open the palette with `Ctrl+K`, and confirm: an unattached agent shows under `Herdr agents`, typing `herdr` brings up both kinds of row, and Enter on an unattached one opens an attached session in the right workspace.

- [ ] **Step 11: Commit**

```bash
devkit run task fmt
git add alacritree/src/app.rs alacritree/src/command_palette.rs
git commit -m "feat(palette): offer unattached herdr agents

An agent nothing is attached to had no palette row at all, so the only
way to reach one was the sidebar. Enter attaches it in the workspace its
working directory matched, which the shared listing already resolved.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 4: Restore the workspace when a shared-view attach fails

Severable — the palette works without it, inheriting exactly what the sidebar does today. Do it because the palette makes an existing papercut easier to hit.

`attach_herdr_agent` returns `true` unconditionally on the shared-view path (`app.rs:1456`), so the `else` branch every caller writes is dead for that mode. When the job later fails, `poll_herdr_attach` raises a dialog and drops `pending.workspace` without touching `current_workspace` (`app.rs:1471-1476`), leaving the user on the target workspace looking at a "no session" placeholder — `ensure_active_session` runs only from `activate_worktree` / `activate_home` (`app.rs:1716`, `:1721`), so nothing spawns a shell to fill it.

Carrying the fallback on `PendingHerdrAttach` fixes it once for all three call sites.

**Files:**
- Modify: `alacritree/src/app.rs:8583-8587` (`PendingHerdrAttach`), `:1419-1457` (`attach_herdr_agent`), `:1462-1479` (`poll_herdr_attach`), `:3034` (sidebar keyboard), `:4841` (sidebar click), `:9580` (palette, from Task 3)

**Interfaces:**
- Produces: `fn attach_herdr_agent(&mut self, ctx: &Context, key: herdr::HerdrKey, pane_id: &str, workspace: WorkspaceKey, previous: WorkspaceKey) -> bool` — one argument wider than Task 3 left it.

- [ ] **Step 1: Add the field**

`app.rs:8583`:

```rust
struct PendingHerdrAttach {
    job: jobs::Job<Result<(String, Vec<String>), String>>,
    key: herdr::HerdrKey,
    workspace: WorkspaceKey,
    /// Where to hand the user back when herdr refuses.  A shared-view
    /// attach answers frames after the switch, so the caller cannot restore
    /// the workspace itself the way a direct attach lets it.
    previous: WorkspaceKey,
}
```

- [ ] **Step 2: Widen `attach_herdr_agent`**

Add `previous: WorkspaceKey` as the last parameter (`app.rs:1419`), and populate the new field at the push (`app.rs:1455`):

```rust
self.pending_herdr_attach.push(PendingHerdrAttach { job, key, workspace, previous });
```

The early-return paths (`herdr_session_for`, the direct attach, the dedupe at `:1444`) ignore it — the caller's own `else` branch still handles those.

- [ ] **Step 3: Restore on both failure arms**

`poll_herdr_attach` (`app.rs:1465-1476`):

```rust
Some(Ok((program, argv))) => {
    let key = pending.key.clone();
    if !self.open_herdr_session(ctx, key, pending.workspace.clone(), program, argv) {
        self.current_workspace = pending.previous;
    }
},
Some(Err(e)) => {
    self.restore_after_failed_attach(&pending);
    self.error_dialog = Some(e);
},
None if pending.job.failed() => {
    self.restore_after_failed_attach(&pending);
    self.error_dialog = Some("the herdr attach did not finish".to_string());
},
```

Three arms, not two. `open_herdr_session` returns a `bool` this call discards (`app.rs:1466`), so a PTY that fails to spawn strands the user exactly as a refusal does.

**Restore only if the user is still where the attach put them.** The job answers frames later, and by then they may have switched away on purpose; assigning `previous` unconditionally yanks them out of whatever they moved to. Hence the helper rather than a bare assignment:

```rust
/// Hand the user back only if they are still sitting on the workspace this
/// attach switched them to.  The job answers frames later, so a switch made
/// in between is theirs and outranks the restore.
fn restore_after_failed_attach(&mut self, pending: &PendingHerdrAttach) {
    if self.current_workspace == pending.workspace {
        self.current_workspace = pending.previous.clone();
    }
}
```

Apply the same guard inside the `Some(Ok(..))` arm by reading `self.current_workspace` before the `open_herdr_session` call, since that call sets it.

- [ ] **Step 4: Pass `previous` at all three call sites**

Sidebar keyboard (`app.rs:3045`) and palette (Task 3, Step 8) already bind `previous` before the call — pass `previous.clone()` and keep their own `else` branch, which still covers the direct-attach refusal.

Sidebar click (`app.rs:4844`) likewise: `self.attach_herdr_agent(ctx, key, &pane_id, ws, previous.clone())`.

- [ ] **Step 5: Build and run the suite**

```bash
devkit run task check
devkit run task test
devkit run task clippy
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
devkit run task fmt
git add alacritree/src/app.rs
git commit -m "fix(herdr): hand back the workspace when an attach fails late

A shared-view attach answers frames after the switch, so it returns true
and the caller's restore never runs. A refusal then left the user on a
workspace with no session and nothing to spawn one, since
ensure_active_session runs only from the activate paths.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 5: The painter draws what the model listed

Behaviour-identical refactor, and the safe ground Task 8 needs. Today the painter drops every non-session entry from the listing while filtering (`app.rs:4040-4045`) because `filtered_rows` cannot emit a `HerdrAgent` row. It collects only `Home`, `Project` and `Worktree` rows out of the model (`:4008-4025`) and then draws *every* entry the listing holds under a surviving workspace (`:4047-4055`, painted at `:4300`).

Once Task 8 makes the model emit a subset of a workspace's children, that gap strands the siblings — the same failure the `retain` exists to prevent, in the opposite direction. Replacing the `retain` with an intersection against the model's own rows closes it. While `filtered_rows` still emits no agents and every session under a surviving workspace, the intersection produces exactly what the `retain` did.

**Files:**
- Modify: `alacritree/src/app.rs:4001-4045`

**Interfaces:**
- Consumes: `sidebar_nav::WorkspaceEntry::row` (`sidebar_nav.rs:44`)
- Produces: nothing new; `listed` is narrowed by membership in `rows` rather than by variant.

- [ ] **Step 1: Collect child rows alongside the workspace rows**

In the `if filtering` block at `app.rs:4008`, replace the `Session | HerdrAgent` arm:

```rust
let mut home_visible = true;
let mut visible_projects: HashSet<PathBuf> = HashSet::new();
let mut visible_worktrees: HashSet<PathBuf> = HashSet::new();
let mut visible_children: HashSet<SidebarRow> = HashSet::new();
if filtering {
    home_visible = false;
    for row in rows {
        match row {
            SidebarRow::Home => home_visible = true,
            SidebarRow::Project(root) => {
                visible_projects.insert(root);
            },
            SidebarRow::Worktree(path) => {
                visible_worktrees.insert(path);
            },
            SidebarRow::Session(_) | SidebarRow::HerdrAgent(..) => {
                visible_children.insert(row.clone());
            },
        }
    }
}
```

`SidebarRow` derives `PartialEq, Eq` (`sidebar_nav.rs:17`) but not `Hash`. Add `Hash` to that derive; `Side` is already `Hash` (`herdr.rs:20`) and `SessionId` and `PathBuf` are too.

- [ ] **Step 2: Replace the strip with an intersection**

`app.rs:4040`:

```rust
// The cursor can only reach a row the nav model listed, so the paint pass
// draws that set and no more — painting a session the filter dropped
// would strand it exactly as painting an unlisted agent would.
let mut listed = self.listed_workspace_rows();
if filtering {
    for entries in listed.values_mut() {
        entries.retain(|entry| visible_children.contains(&entry.row()));
    }
}
```

Delete the six comment lines at `:4034-4039`, from the line starting `sidebar_nav::filtered_rows` never emits, through `positions the nav model gave them.` — they describe a rule this replaces. The three lines above them (`:4031-4033`, about snapshotting attention up-front) are unrelated and stay.

- [ ] **Step 3: Build and run the suite**

```bash
devkit run task check
devkit run task test
```

Expected: PASS, including `steady_state.rs` — the new `HashSet` is built inside the `filtering` branch, which never runs on an unchanged non-filtering frame.

- [ ] **Step 4: Smoke-test by hand**

Open the sidebar, type a query that matches a worktree holding sessions, and confirm the sessions still render under it exactly as before.

- [ ] **Step 5: Commit**

```bash
devkit run task fmt
git add alacritree/src/app.rs alacritree/src/sidebar_nav.rs
git commit -m "refactor(sidebar): paint the child rows the nav model listed

Dropping non-session entries was a stand-in for the model not emitting
them. Intersecting against the model's own rows says the actual rule —
paint what the cursor can reach — and holds when the model starts
emitting a subset of a workspace's children.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 6: Split the row predicates

Behaviour-identical. Today `current_project_rows` fuses three dimensions into one bool per worktree (`app.rs:2787`):

```rust
let mut worktree = |_p: &Project, wt: &Worktree| {
    worktree_matches.get(&wt.path).copied().unwrap_or(false)
        && toggles_pass(&Some(wt.path.clone()))
        && worktree_pr_passes(any_pr, &pr_matches, &wt.path)
};
```

A child match cannot be OR-ed into that without also bypassing the toggle and PR gates. Splitting the name test from the gate now, with the child predicate wired as `None`, leaves Task 8 a one-line change.

**This task is not behaviour-neutral, despite `child: None`.** The old `push_session_rows` (`sidebar_nav.rs:340-346`) emitted only `WorkspaceEntry::session` rows; the new `children` helper emits `WorkspaceEntry::row` for every entry, agents included. Combined with Task 5's intersection, that means herdr agent rows start appearing under name-matched workspaces while filtering — here, not in Task 8. That is the intended end state arriving early, and it is coherent: the painter draws what the model listed, the reconciler already watches `herdr_generation` (`app.rs:2894`), and `rendered_differs` (`herdr.rs:770`) covers title, kind and cwd. What Task 8 still adds is the child predicate that surfaces a workspace *because* an agent matched. Write the commit message for what this actually lands.

**Files:**
- Modify: `alacritree/src/sidebar_nav.rs:311-317` (`RowPredicates`), `:328-362` (`filtered_rows`)
- Modify: `alacritree/src/app.rs:2716-2795` (`current_project_rows`)
- Test: `alacritree/src/sidebar_nav.rs` `mod tests` — the six existing `RowPredicates` literals at `:672`, `:687`, `:701`, `:715`, `:832` and `:851` update to the new shape

**Interfaces:**
- Produces:

```rust
pub struct RowPredicates<'a> {
    pub home_gate: bool,
    pub home_name: bool,
    pub project_self: &'a dyn Fn(&Project) -> bool,
    pub gate: &'a dyn Fn(&WorkspaceKey) -> bool,
    pub name: &'a mut dyn FnMut(&Project, &Worktree) -> bool,
    pub child: Option<&'a mut dyn FnMut(&WorkspaceKey, &WorkspaceEntry) -> bool>,
}
```

- [ ] **Step 1: Claim the file**

```bash
DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm acquire \
  "$(pwd)/alacritree/src/sidebar_nav.rs" --note "herdr palette: predicate split"
```

- [ ] **Step 2: Update the existing tests to the new shape**

The four `filtered_rows_*` tests build `RowPredicates` literally, so they must move with it. `filtered_rows_keeps_projects_with_matching_worktrees_and_forces_expansion` (`:670`) becomes:

```rust
#[test]
fn filtered_rows_keeps_projects_with_matching_worktrees_and_forces_expansion() {
    let projects = vec![project("/a", false, &["/a/wt1", "/a/wt2"])];
    let preds = RowPredicates {
        home_gate: true,
        home_name: true,
        project_self: &|_p| false,
        gate: &|_ws| true,
        name: &mut |_p, wt| wt.path == PathBuf::from("/a/wt1"),
        child: None,
    };
    assert_eq!(filtered_rows(&projects, &no_sessions(), preds), vec![
        SidebarRow::Home,
        SidebarRow::Project(PathBuf::from("/a")),
        SidebarRow::Worktree(PathBuf::from("/a/wt1")),
    ]);
}
```

Apply the same mechanical shape to `filtered_rows_keeps_a_self_matching_header_without_its_worktrees` (`:685`), `filtered_rows_drops_projects_matching_neither_self_nor_worktrees` (`:713`) and `filtered_rows_appends_session_rows_to_surviving_workspace_rows` (`:825`), each keeping its own `project_self` and `name` closures with `gate: &|_ws| true` and `child: None`.

`filtered_rows_hides_home_session_rows_with_home` (`:851`) is the sixth, and the easiest to miss because its `RowPredicates` is a one-line literal rather than a `let` block:

```rust
let preds = RowPredicates {
    home_gate: true,
    home_name: false,
    project_self: &|_| true,
    gate: &|_ws| true,
    name: &mut |_, _| true,
    child: None,
};
```

Its assertion (`!rows.contains(&SidebarRow::Session(1))`) still holds: Home is dropped by `home_name`, so its session rows never reach the output.

`filtered_rows_drops_home_when_it_fails_the_predicate` (`:699`) is the one that splits in two — the old `home: false` had two possible causes. Replace it with:

```rust
#[test]
fn filtered_rows_drops_home_when_its_name_does_not_match() {
    let projects = vec![project("/a", true, &["/a/wt1"])];
    let preds = RowPredicates {
        home_gate: true,
        home_name: false,
        project_self: &|p| p.root == PathBuf::from("/a"),
        gate: &|_ws| true,
        name: &mut |_p, _wt| true,
        child: None,
    };
    assert_eq!(filtered_rows(&projects, &no_sessions(), preds), vec![
        SidebarRow::Project(PathBuf::from("/a")),
        SidebarRow::Worktree(PathBuf::from("/a/wt1"))
    ]);
}

/// A toggle narrows the tree whether or not anything matches by name, so the
/// gate drops Home on its own.
#[test]
fn filtered_rows_drops_home_when_it_fails_the_gate() {
    let projects = vec![project("/a", true, &["/a/wt1"])];
    let preds = RowPredicates {
        home_gate: false,
        home_name: true,
        project_self: &|p| p.root == PathBuf::from("/a"),
        gate: &|_ws| true,
        name: &mut |_p, _wt| true,
        child: None,
    };
    assert_eq!(filtered_rows(&projects, &no_sessions(), preds), vec![
        SidebarRow::Project(PathBuf::from("/a")),
        SidebarRow::Worktree(PathBuf::from("/a/wt1"))
    ]);
}
```

- [ ] **Step 3: Write the new emission-rule tests**

Same module. These are the rule this task exists to build; Task 8 wires a real predicate into it. `sessions_only` (`:383`) builds a session-only listing, so agent entries go in directly.

```rust
fn agent_listing() -> ListedRows {
    ListedRows::from([(
        ws("/a/wt1"),
        vec![
            WorkspaceEntry::Session(1),
            WorkspaceEntry::Agent(Side::Native, "term_a".into()),
        ],
    )])
}

/// Searching for a child hands back that child, not its workspace plus every
/// sibling, which would be the opposite of searching.
#[test]
fn filtered_rows_surfaces_a_workspace_for_a_matching_child_alone() {
    let projects = vec![project("/a", false, &["/a/wt1"])];
    let preds = RowPredicates {
        home_gate: true,
        home_name: false,
        project_self: &|_p| false,
        gate: &|_ws| true,
        name: &mut |_p, _wt| false,
        child: Some(&mut |_ws, e| *e == WorkspaceEntry::Agent(Side::Native, "term_a".into())),
    };
    assert_eq!(filtered_rows(&projects, &agent_listing(), preds), vec![
        SidebarRow::Project(PathBuf::from("/a")),
        SidebarRow::Worktree(PathBuf::from("/a/wt1")),
        SidebarRow::HerdrAgent(Side::Native, "term_a".into()),
    ]);
}

/// A workspace matching by its own name keeps every child, agents included.
/// The child rule narrows which workspaces surface, never which children a
/// surfaced workspace shows.
#[test]
fn filtered_rows_keeps_every_child_of_a_name_matched_workspace() {
    let projects = vec![project("/a", false, &["/a/wt1"])];
    let preds = RowPredicates {
        home_gate: true,
        home_name: false,
        project_self: &|_p| false,
        gate: &|_ws| true,
        name: &mut |_p, _wt| true,
        child: Some(&mut |_ws, _e| false),
    };
    assert_eq!(filtered_rows(&projects, &agent_listing(), preds), vec![
        SidebarRow::Project(PathBuf::from("/a")),
        SidebarRow::Worktree(PathBuf::from("/a/wt1")),
        SidebarRow::Session(1),
        SidebarRow::HerdrAgent(Side::Native, "term_a".into()),
    ]);
}

/// The gate is the only way in, so a toggle keeps narrowing the tree even
/// when a child would otherwise surface its workspace.
#[test]
fn filtered_rows_does_not_let_a_child_match_bypass_the_gate() {
    let projects = vec![project("/a", false, &["/a/wt1"])];
    let preds = RowPredicates {
        home_gate: false,
        home_name: false,
        project_self: &|_p| false,
        gate: &|_ws| false,
        name: &mut |_p, _wt| false,
        child: Some(&mut |_ws, _e| true),
    };
    assert!(filtered_rows(&projects, &agent_listing(), preds).is_empty());
}
```

- [ ] **Step 4: Run them all to verify they fail**

```bash
cargo nextest run -p alacritree --locked filtered_rows
```

Expected: FAIL to compile — `struct 'RowPredicates' has no field named 'home_gate'`, and no field named `child`. That covers both the updated tests and the three new ones.

- [ ] **Step 5: Rewrite `RowPredicates` and `filtered_rows`**

`sidebar_nav.rs:311`:

```rust
/// What decides whether a row survives an active filter.  The gate (toggle
/// and PR dimensions) is separate from the name test because a child match
/// surfaces a workspace through the gate and never around it.  `name` and
/// `child` are `FnMut` because the fuzzy matcher they wrap needs `&mut self`.
pub struct RowPredicates<'a> {
    pub home_gate: bool,
    pub home_name: bool,
    pub project_self: &'a dyn Fn(&Project) -> bool,
    pub gate: &'a dyn Fn(&WorkspaceKey) -> bool,
    pub name: &'a mut dyn FnMut(&Project, &Worktree) -> bool,
    /// `None` while the query is empty, which is what keeps a bare toggle
    /// from surfacing every workspace that holds any child at all.
    pub child: Option<&'a mut dyn FnMut(&WorkspaceKey, &WorkspaceEntry) -> bool>,
}

/// Render-order rows under an active filter. Projects are force-expanded (a
/// filter that hides its own results is useless); a header survives when it
/// matches itself or keeps at least one visible worktree.  A workspace that
/// matched by name keeps all its children; one surfaced only because a child
/// matched keeps just the matching ones.
pub fn filtered_rows(
    projects: &[Project],
    listed: &ListedRows,
    mut preds: RowPredicates<'_>,
) -> Vec<SidebarRow> {
    /// The rows a surviving workspace contributes, and whether any child
    /// matched.  A name match takes every child; otherwise only the matches
    /// survive, so searching for a child does not hand back its siblings.
    fn children(
        preds: &mut RowPredicates<'_>,
        listed: &ListedRows,
        ws: &WorkspaceKey,
        name_matched: bool,
    ) -> (Vec<SidebarRow>, bool) {
        let Some(entries) = listed.get(ws) else {
            return (Vec::new(), false);
        };
        let Some(child) = preds.child.as_mut() else {
            return (
                if name_matched {
                    entries.iter().map(WorkspaceEntry::row).collect()
                } else {
                    Vec::new()
                },
                false,
            );
        };
        // `filter` hands the closure `&&WorkspaceEntry`, so the pattern
        // destructures one layer off before the predicate sees it.
        let matching: Vec<SidebarRow> =
            entries.iter().filter(|&e| child(ws, e)).map(WorkspaceEntry::row).collect();
        let any = !matching.is_empty();
        if name_matched {
            (entries.iter().map(WorkspaceEntry::row).collect(), any)
        } else {
            (matching, any)
        }
    }

    let mut rows = Vec::new();
    // Read before the `&mut preds` borrow: an argument list evaluates left to
    // right, so a field read after it would be a use of a borrowed value.
    let (home_gate, home_name) = (preds.home_gate, preds.home_name);
    let (home_children, home_child_matched) = children(&mut preds, listed, &None, home_name);
    if home_gate && (home_name || home_child_matched) {
        rows.push(SidebarRow::Home);
        rows.extend(home_children);
    }
    for p in projects {
        let self_matches = (preds.project_self)(p);
        let mut visible_worktrees: Vec<SidebarRow> = Vec::new();
        for wt in &p.worktrees {
            let ws = Some(wt.path.clone());
            if !(preds.gate)(&ws) {
                continue;
            }
            let name_matched = (preds.name)(p, wt);
            let (child_rows, child_matched) = children(&mut preds, listed, &ws, name_matched);
            if !name_matched && !child_matched {
                continue;
            }
            visible_worktrees.push(SidebarRow::Worktree(wt.path.clone()));
            visible_worktrees.extend(child_rows);
        }
        if self_matches || !visible_worktrees.is_empty() {
            rows.push(SidebarRow::Project(p.root.clone()));
            rows.extend(visible_worktrees);
        }
    }
    rows
}
```

Delete the `push_session_rows` inner fn and its comment — the `children` helper replaces both.

- [ ] **Step 6: Update `current_project_rows` to the new shape**

`app.rs:2779-2795`:

```rust
let gate = |key: &WorkspaceKey| {
    project_toggles_pass(
        apply,
        toggle_sessions,
        self.workspace_has_sessions(key),
        toggle_attention,
        self.workspace_needs_attention(key),
    ) && key.as_deref().is_none_or(|path| worktree_pr_passes(any_pr, &pr_matches, path))
};
let project_self =
    |p: &Project| !any_toggle && project_matches.get(&p.root).copied().unwrap_or(false);
let mut name = |_p: &Project, wt: &Worktree| {
    worktree_matches.get(&wt.path).copied().unwrap_or(false)
};
sidebar_nav::filtered_rows(&self.projects, &listed, sidebar_nav::RowPredicates {
    home_gate: gate(&None),
    home_name: home_matches,
    project_self: &project_self,
    gate: &gate,
    name: &mut name,
    child: None,
})
```

Delete the now-unused `toggles_pass` closure and the `home` binding.

- [ ] **Step 7: Run the suite**

```bash
devkit run task check
devkit run task test
devkit run task clippy
```

Expected: PASS. Every existing sidebar filter and cursor test is the regression net for "behaviour-identical".

- [ ] **Step 8: Commit**

```bash
devkit run task fmt
git add alacritree/src/sidebar_nav.rs alacritree/src/app.rs
git commit -m "feat(sidebar): show agent rows under a matched workspace

Fusing the toggle and PR dimensions into the worktree bool leaves nowhere
for a child match to enter: OR-ing one in would bypass the toggles too.
Splitting them keeps the gate on every path a row can survive by, and the
child rows a surviving workspace contributes are now every entry it holds
rather than its sessions alone, so filtering no longer hides its agents.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 7: The reconciler watches session titles

Behaviour-identical: nothing matches on a title yet, but the focus reconciler starts watching it, so Task 8 cannot ship a row set that goes stale.

`SessionInput` is `{workspace, id, attention}` (`sidebar_focus.rs:288`) and the per-frame comparison reads those three (`:490`). `reconcile_sidebar_focus` returns early when the inputs match (`app.rs:2897`) and the paint path reuses `sidebar_rows_cache` while filtering (`app.rs:3970`). Once titles enter the filter, a PTY title change — every prompt, in many shells — would neither add nor remove a row until something unrelated moved.

Agent names need nothing: `ObservedInputs` already carries `herdr_generation` (`app.rs:2894`), bumped only when a rendered field changes (`herdr.rs:478`).

The title is stamped empty when no query is live, so a non-filtering frame costs exactly what it costs today.

**Files:**
- Modify: `alacritree/src/sidebar_focus.rs:288-294` (`SessionInput`), `:409-448` (`capture`), `:486-499` (`matches`), `:664` (test helper)
- Modify: `alacritree/src/app.rs:2814-2821` (`session_inputs`), `:2825` and `:2880` (both call sites)
- Modify: `alacritree/src/steady_state.rs:107` — its `inputs` helper builds `SessionInput` literally and stops compiling without the new field. Add `title: ""`. The allocation assertions themselves survive: the comparison is a `&String`-against-`&str` borrow, and `capture` runs only on the rebuild path.

**Interfaces:**
- Produces:
  - `pub struct SessionInput<'a> { pub workspace: &'a WorkspaceKey, pub id: SessionId, pub attention: bool, pub title: &'a str }`
  - `fn AlacritreeApp::session_inputs(&self, titles: bool) -> impl Iterator<Item = sidebar_focus::SessionInput<'_>>`

- [ ] **Step 1: Write the failing test**

`sidebar_focus.rs` already has `every_observed_input_in_isolation_triggers_a_rebuild` (`:663`). Extend its local helper and add a case:

```rust
let session = |ws: &'static WorkspaceKey, id, attention, title| SessionInput {
    workspace: ws,
    id,
    attention,
    title,
};
```

then, beside the existing per-input assertions:

```rust
// A title-matched row appears and disappears as the title changes, so the
// projection has to rebuild when one does.
assert!(!base.matches(&[], [session(&HOME, 1, false, "nvim")].into_iter(), ui("", 0)));
```

with `base` captured from `session(&HOME, 1, false, "")`.

- [ ] **Step 2: Run it to verify it fails**

```bash
cargo nextest run -p alacritree --locked every_observed_input_in_isolation_triggers_a_rebuild
```

Expected: FAIL to compile — `struct 'SessionInput' has no field named 'title'`.

- [ ] **Step 3: Add the field**

`sidebar_focus.rs:288`:

```rust
/// One live session, borrowed for the per-frame comparison.
#[derive(Debug, Clone, Copy)]
pub struct SessionInput<'a> {
    pub workspace: &'a WorkspaceKey,
    pub id: SessionId,
    pub attention: bool,
    /// Empty unless a query is live: a title only changes the row set while
    /// the filter is matching on one, and PTY titles change every prompt.
    pub title: &'a str,
}
```

- [ ] **Step 4: Store and compare it**

In `capture` (`:409`), extend the stored session tuple to carry an owned `String`; in `matches` (`:486`), extend the comparison:

```rust
Some((ws, id, attention, title))
    if ws == s.workspace
        && *id == s.id
        && *attention == s.attention
        && title == s.title =>
```

Comparing `&String` against `&str` borrows rather than allocates, so the allocation-free contract `steady_state.rs` asserts holds.

- [ ] **Step 5: Stamp it at the source**

`app.rs:2814`:

```rust
/// Live sessions borrowed for the unchanged-inputs check, which runs on
/// every frame and must not allocate.  `titles` is off unless a query is
/// live, so a shell repainting its prompt does not invalidate a projection
/// no title can change.
fn session_inputs(&self, titles: bool) -> impl Iterator<Item = sidebar_focus::SessionInput<'_>> {
    self.sessions.iter().map(move |s| sidebar_focus::SessionInput {
        workspace: &s.working_directory,
        id: s.id,
        attention: s.needs_attention,
        title: if titles { &s.title } else { "" },
    })
}
```

Both call sites (`app.rs:2825` and the `matches` call around `:2880`) pass `!self.project_filter.query().is_empty()`.

- [ ] **Step 6: Run the suite**

```bash
devkit run task check
devkit run task test
```

Expected: PASS, `steady_state.rs` included.

- [ ] **Step 7: Commit**

```bash
devkit run task fmt
git add alacritree/src/sidebar_focus.rs alacritree/src/app.rs
git commit -m "refactor(sidebar): observe session titles while a query is live

The reconciler's early-out reads workspace, id and attention, so a row
the filter selected by title would neither appear nor vanish until
something unrelated moved. Stamping the title empty with no query keeps
every other frame at today's cost.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 8: The sidebar filter finds agents and session titles

The feature lands. Everything before this made the ground safe: the painter draws what the model listed (Task 5), the gate survives a child match (Task 6), the reconciler watches titles (Task 7).

Sessions and agents both match on their display name — for a session that is `session_row_name`, for an agent it is herdr's title and kind — so `claude` finds every Claude pane and a pane titled `fix the wrap bug` is found by its words.

**Files:**
- Modify: `alacritree/src/app.rs:2716-2795` (`current_project_rows`), `:4020-4022` (the stale painter comment)

**Interfaces:**
- Consumes: `RowPredicates.child` (Task 6), `session_row_name` (`app.rs:8003`), `find_herdr_agent` (`app.rs:8955`), `herdr_row_name` (`app.rs:7382`)
- Produces: nothing new; `child` goes from `None` to `Some`.

- [ ] **Step 1: Confirm the emission rule is already green**

```bash
cargo nextest run -p alacritree --locked filtered_rows
```

Expected: PASS. Task 6 built and tested the rule; this task supplies the predicate that makes it fire against real names. There is no unit-level RED available here — `current_project_rows` takes `&mut self` on `AlacritreeApp`, which has no test fixture — so the gate for this task is the manual pass in Step 6 plus the existing suite.

- [ ] **Step 2: Precompute the child names**

In `current_project_rows` (`app.rs:2716`), after the `worktree_matches` map (`:2751`) and before the closures. The matcher needs `&mut self.project_filter` while the names need `&self`, so every name is materialised first — the same reason `worktree_matches` is precomputed, with more force, since `session_row_name` reaches through two `&self` helpers.

```rust
// Child names are resolved before the matcher borrows the filter: the
// names come off `&self` helpers and the matcher wants `&mut
// self.project_filter`, so the two cannot be live at once.  Skipped
// outright with an empty query, where `matches` answers true for
// everything and every workspace holding any child would surface.
let child_matches: HashMap<SidebarRow, bool> = if self.project_filter.query().is_empty() {
    HashMap::new()
} else {
    let names: Vec<(SidebarRow, String)> = listed
        .values()
        .flatten()
        .map(|entry| {
            let name = match entry {
                sidebar_nav::WorkspaceEntry::Session(id) => self
                    .sessions
                    .iter()
                    .find(|s| s.id == *id)
                    .map(|s| {
                        let activity = herdr_backed_activity(
                            s.activity(),
                            self.session_herdr_status(s),
                        );
                        session_row_name(&s.title, activity, self.session_herdr_agent(s))
                    })
                    .map(|n| match n.context {
                        Some(context) => format!("{} {}", n.text, context),
                        None => n.text,
                    })
                    .unwrap_or_default(),
                sidebar_nav::WorkspaceEntry::Agent(side, terminal_id) => self
                    .find_herdr_agent(side, terminal_id)
                    .map(|a| {
                        // Mirror the painted fallback in
                        // `HerdrRowData::from_agent` (`app.rs:7344`): an agent
                        // with neither title nor kind paints the terminal id's
                        // tail, so that is the only name a query can match.
                        let name = herdr_row_name(a)
                            .map(|n| n.text)
                            .or_else(|| a.kind.clone())
                            .unwrap_or_else(|| {
                                let id = &a.terminal_id;
                                let skip = id.chars().count().saturating_sub(6);
                                id.chars().skip(skip).collect()
                            });
                        match &a.kind {
                            Some(kind) if *kind != name => format!("{name} {kind}"),
                            _ => name,
                        }
                    })
                    .unwrap_or_default(),
            };
            (entry.row(), name)
        })
        .collect();
    let filter = &mut self.project_filter;
    names.into_iter().map(|(row, name)| (row, filter.matches(&name))).collect()
};
```

- [ ] **Step 3: Wire the predicate**

Replace `child: None` in the `RowPredicates` literal:

```rust
let matched_children = !child_matches.is_empty();
let mut child = |_ws: &WorkspaceKey, entry: &sidebar_nav::WorkspaceEntry| {
    child_matches.get(&entry.row()).copied().unwrap_or(false)
};
let child: Option<&mut dyn FnMut(&WorkspaceKey, &sidebar_nav::WorkspaceEntry) -> bool> =
    if matched_children { Some(&mut child) } else { None };
```

and pass `child` straight into the literal. Spell the `Option` out rather than reaching for `then_some`: that would build the `&mut` borrow whether or not it is used, and the trait object needs a named type to coerce to anyway.

An empty map means an empty query, which is what keeps a bare toggle from surfacing every workspace that holds any child: `PanelFilter::matches` returns `true` for an empty query (`panel_filter.rs:159`) while `is_filtering` is true on toggles alone (`panel_filter.rs:104`).

- [ ] **Step 4: Run everything**

```bash
devkit run task check
devkit run task test
devkit run task clippy
```

Expected: PASS.

- [ ] **Step 5: Smoke-test by hand**

With a herdr server running and at least one unattached agent:

1. Open the sidebar, press `/`, type part of an agent's title. The agent's row appears under its workspace, and its sibling sessions do not.
2. Type part of a worktree's name. Every child of that worktree appears, agents included.
3. Type part of a session's title. Only that session appears under its workspace. Pick a workspace holding at least two children: `workspace_entries` (`app.rs:7492`) lists shell rows only once a workspace has two or more children, unless `session_display.sidebar_always` is on, so a lone shell has no row to match in the first place.
4. Clear the query, turn on the attention toggle. The tree narrows exactly as it did before this branch — no workspace surfaces merely for holding a child.
5. With a query live, let a shell repaint its prompt and confirm the row list keeps up.
6. Arrow through the filtered rows and confirm the cursor reaches every row on screen and stops at none that is missing.

- [ ] **Step 6: Commit**

```bash
devkit run task fmt
git add alacritree/src/app.rs
git commit -m "feat(sidebar): match sessions and herdr agents by name

Filtering used to delete every agent row and never matched a session
title, so a search could only reach workspaces. Both now match on the
name their row paints, and a workspace surfaced only by a child shows
just the matching children.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 9: Document both surfaces

**Files:**
- Modify: `docs/alacritree.md` (the `### herdr agents` section at `:77`, and the `search_scope` config comment at `:439`)

- [ ] **Step 1: Claim the file**

```bash
DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm acquire \
  "$(pwd)/docs/alacritree.md" --note "herdr palette: user docs"
```

- [ ] **Step 2: Add a bullet to the herdr section**

After the **Attaching** bullet (`:81`), soft-wrapped as one line per bullet:

```markdown
- **Finding an agent by name.** The command palette (`Ctrl+K`) lists agents alongside sessions: an attached one sits under *Open sessions*, named the way its sidebar row is, and one nothing is attached to sits under *Herdr agents*, where Enter attaches it in the workspace its working directory matched. Typing `herdr` brings up both kinds. The sidebar's own search (`/`) matches agents and session titles too, so a query naming one agent shows that agent rather than its whole workspace; a query naming the workspace still shows everything under it. A session the sidebar does not list has no row to match — by default a workspace's only shell is folded into its workspace row, which `session_display.sidebar_always` turns off.
```

- [ ] **Step 3: Extend the search_scope note**

The comment block at `:439-444` says what a query narrows but not what it matches on. Insert two lines under `# active toggle filters`, keeping the existing column alignment:

```
search_scope       = "filtered"  # whether a sidebar search is confined by the
                                 # active toggle filters; a query matches
                                 # project, worktree, session and herdr agent
                                 # names alike
                                 # "filtered" (default): a query narrows what
                                 # the toggles already allow
                                 # "all": a query reaches every row; the
                                 # toggles resume when it empties
```

- [ ] **Step 4: Commit**

```bash
git add docs/alacritree.md
git commit -m "docs: describe herdr rows in the palette and the sidebar search

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Verification before the PR

- [ ] `devkit run task fmt`, `devkit run task check`, `devkit run task test`, `devkit run task clippy` all clean.
- [ ] The base has not moved out from under the branch: `git -C <worktree> merge-base --is-ancestor <recorded base> origin/feat/config-schema-defaults`. If it fails, `git -C <worktree> rebase --onto origin/feat/config-schema-defaults <recorded base> feat/herdr-palette`.
- [ ] The full manual pass from Task 8, Step 6, plus Task 3, Step 10.
- [ ] With `[integrations.herdr] enabled = false`, the palette shows no `Herdr agents` heading and the sidebar filter behaves as it did before the branch.
- [ ] With `show_unmatched = false`, an agent matching no worktree appears in neither the sidebar nor the palette.

## Open questions for the reviewer

1. Task 4 fixes a bug in two existing sidebar paths that this branch did not introduce. Keeping it here is one commit; splitting it into its own issue keeps this diff to the palette. Either is defensible.
2. Nothing here surfaces a herdr *session* as distinct from its agents. `herdr.rs:280` already reads `session list` for the attach gesture, so a "Herdr sessions" row set is reachable later; this plan stops at agents, which is what the sidebar models.
