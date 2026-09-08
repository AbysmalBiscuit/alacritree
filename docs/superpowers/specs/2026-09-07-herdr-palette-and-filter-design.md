# Herdr in the palette and the sidebar filter design

**Goal:** reach a herdr agent by typing its name. From the command palette, an attached agent is focused and an unattached one is attached; from the sidebar filter, both appear as rows instead of vanishing. Session titles become searchable in the sidebar filter for the same reason.

**Issues:** [#77](https://github.com/AbysmalBiscuit/alacritree/issues/77), sub-issue of [#43](https://github.com/AbysmalBiscuit/alacritree/issues/43).

**Branch:** `feat/herdr-palette`, marker `[15]`. Stacked on `feat/config-schema-defaults` (`[14]`), which is stacked on PR 215. Read the tip fresh at setup time.

**Platform:** all. The attach gesture already handles the native-Windows fallback through `attaches_directly`; nothing here adds a per-platform path.

**Config:** no new keys. The herdr rows exist only while `[integrations.herdr] enabled` is true and a herdr server answers, so the feature is already gated by the key that gates herdr. The sidebar-filter half only adds rows to a filter that already had a query typed into it, so it needs no opt-in of its own — section 3 makes that a property of the predicate rather than a claim.

## Context

### The palette has herdr rows already, and they are illegible

`palette_items` (`app.rs:9515`) walks `self.sessions` and emits one `OpenSessions` row each:

```rust
for session in &self.sessions {
    let ws = self.workspace_label(&session.working_directory);
    items.push(PaletteItem::session(session.id, session.title.clone(), format!("session · {ws}")));
}
```

A herdr-attached session is in that list, because it is an ordinary session with a `herdr_key` set. Its `primary` is `session.title` — the PTY title. For a direct attach that is the agent's pane title; for a shared-view attach it is herdr's whole-window title, which names no agent at all. The sidebar solved this: `session_row_name` (`app.rs:8003`) prefers what herdr reports over what the PTY says. The palette never learned.

Unattached agents have no palette row at all.

### The attach gesture is reusable, but its refusal path is one-sided

`attach_herdr_agent` (`app.rs:1419`) takes `(ctx, key, pane_id, workspace)`: an already-attached agent is focused rather than re-attached (`herdr_session_for`), a direct attach spawns the client against the pane id, and a shared-view attach runs the two-call focus-then-name gesture on the job pool so the click never holds a frame.

Both sidebar entry points call it the same way — replace `current_workspace`, attach, restore on refusal (`app.rs:3034`, `app.rs:4841`). That restore only ever fires for a direct attach. The shared-view path pushes onto `pending_herdr_attach` and returns `true` unconditionally (`app.rs:1456`), and when the job later fails, `poll_herdr_attach` sets `error_dialog` and drops `pending.workspace` without touching `current_workspace` (`app.rs:1471-1476`). The user is left on the workspace the attach was for, looking at a "no session" placeholder — `ensure_active_session` runs only from `activate_worktree` / `activate_home` (`app.rs:1716`, `:1721`), so nothing spawns a shell to fill it.

Pre-existing, in both sidebar paths. Section 2 fixes it rather than inheriting it.

### Polling does not depend on the sidebar

`poll_herdr_endpoints` runs at `app.rs:10689`, ahead of the sidebar draw at `:10738`, gated only on `enabled`. Palette rows stay current with the sidebar hidden.

### The sidebar filter deletes herdr rows

This is worse than a missing match. Turning on the filter makes every herdr agent disappear, by `app.rs:4040`:

```rust
let mut listed = self.listed_workspace_rows();
if filtering {
    for entries in listed.values_mut() {
        entries.retain(|entry| entry.session().is_some());
    }
}
```

The reason is upstream of the painter. `filtered_rows` (`sidebar_nav.rs:328`) cannot emit a `HerdrAgent` row, and its own comment says why:

```
// Only session rows survive: a `HerdrAgent` row is keyed by `(Side,
// String)` and carries no display name, so the filter has nothing to
// match it on and a painted one would have no cursor path to reach it.
```

Painting a row the cursor model does not list would strand it, so the painter strips instead. Both halves have to move together.

### Sessions are not name-matched either

`current_project_rows` (`app.rs:2716`) precomputes fuzzy results for project display names and worktree names only (`app.rs:2735`, `:2744`), and `push_session_rows` then appends *every* session under a surviving workspace. Typing a session's title finds nothing today. The gap is not herdr-specific.

## 1. One agent listing, two surfaces

The agent walk inside `listed_workspace_rows` (`app.rs:8887`) applies three filters in sequence: `herdr::unattached` against the claimed keys, `herdr::match_workspace` for the bucket, and `show_unmatched` for whether an unbucketed agent survives.

Extract it as a free function over slices rather than a method:

```rust
/// Every herdr agent no session holds, with the workspace it belongs under.
/// The sidebar and the palette both read this, so an agent hidden from one is
/// hidden from the other by construction rather than by two filters agreeing.
fn listed_herdr_agents<'a>(
    endpoints: &'a [HerdrEndpoint],
    claimed: &[herdr::HerdrKey],
    workspaces: &[WorkspaceKey],
    show_unmatched: bool,
) -> Vec<(WorkspaceKey, &'a herdr::Side, &'a herdr::Agent)>
```

`listed_workspace_rows` buckets the result; section 2 maps it to palette rows. That is what makes "the palette honours `show_unmatched`" a property of the code rather than a claim about two copies.

The signature is a free function because there is no `AlacritreeApp` test fixture and building one is not on this branch's budget. `Session::new` opens a PTY (`session.rs:34`), and `EndpointCache::agents` is private with no constructor but `poll` (`herdr.rs:451`). Every existing test near this code exercises a free function over slices — `workspace_entries` at `app.rs:11810` is the pattern. Taking slices makes section 1's own guarantee testable, which is the point of extracting it.

## 2. Palette rows

### Attached agents stay under Open sessions

An attached agent *is* an open session, and Enter focuses it exactly as it focuses any session. The distinction a heading should carry is open against not-open, so splitting attached agents under a second heading would mean knowing which heading to look under before looking.

What changes is the label. Where `self.session_herdr_agent(session)` answers, the row takes the sidebar's name and a herdr-flavoured secondary:

```
fix the wrap bug        herdr · claude · working · alacritree / feat-x
nvim config             session · Home
```

The name comes from `session_row_name` (`app.rs:8003`), which is what the sidebar paints for the same row. Not `HerdrRowData::from_agent` (`app.rs:7336`): that chain is for an *un*attached agent, which has no PTY of its own, so it falls back to the kind and then the terminal id's tail. An attached session does have a PTY, and `session_row_name` falls back to its title with the kind as context. For a shared-view attach herdr reports no title for, the two disagree — `claude` against the pane's own title — and the sidebar's answer is the right one.

### Unattached agents get one new section

`PaletteSection::HerdrAgents`, titled "Herdr agents", built right after the session block so an empty query orders actions, profiles, sessions, herdr agents, workspaces, worktrees. `PaletteSection::title` (`command_palette.rs:56`) is an exhaustive match and gains an arm; its only other consumer is `paint_palette_section` (`app.rs:9469`), which goes through `title()`.

```
review the schema       claude · idle · attach · wsl:Ubuntu
scratch                 codex · blocked · attach · Home
```

The side appears only for a WSL endpoint. On a Linux or macOS host there is only ever `Side::Native` and the string would be noise on every row; on a Windows host with both, it is what tells two same-named agents apart.

The literal word `herdr` goes into the search haystack without being painted, the way `PaletteItem::profile` (`command_palette.rs:168`) already folds `SpawnProfileN` in. `PaletteItem::new` builds `search` from `primary`, `secondary` and `keys` and the field is private (`command_palette.rs:126`), so this is a new constructor beside `session` rather than a mutation from `app.rs`:

```rust
/// A row whose haystack carries `herdr` even though no column paints it, so
/// the integration's name finds its rows.
pub fn herdr_agent(attach: HerdrAttach, primary: String, secondary: String) -> Self
```

Typing `herdr` then pulls up both the attached and the unattached rows, which is what makes one heading enough. The attached rows come along because their secondary already starts with `herdr ·`.

### Enter

```rust
/// Attach to a herdr agent no session holds, in the workspace its working
/// directory matched.
AttachHerdrAgent(HerdrAttach),

#[derive(Debug, Clone, PartialEq)]
pub struct HerdrAttach {
    pub key: herdr::HerdrKey,
    pub pane_id: String,
    pub workspace: WorkspaceKey,
}
```

`command_palette.rs` already imports `crate::app::WorkspaceKey` and `crate::session::SessionId`, so `herdr::HerdrKey` is the same kind of dependency. The derives match `PaletteAction`'s own (`command_palette.rs:24`), which are `Debug, Clone, PartialEq` — no `Eq`, no `Hash`.

Dispatch in `run_palette_action` (`app.rs:9580`):

```rust
PaletteAction::AttachHerdrAgent(a) => {
    let previous = std::mem::replace(&mut self.current_workspace, a.workspace.clone());
    if self.attach_herdr_agent(ctx, a.key, &a.pane_id, a.workspace, previous.clone()) {
        self.focus_terminal();
    } else {
        self.current_workspace = previous;
    }
}
```

The extra argument is the fix for the one-sided refusal. `attach_herdr_agent` takes the workspace to fall back to and stores it on `PendingHerdrAttach`; `poll_herdr_attach`'s two failure arms restore `current_workspace` from it before raising the dialog, and the success arm drops it. Both sidebar call sites pass their own `previous` and lose their manual restore, so one path handles the refusal instead of two handling half of it.

This is severable. Without it the palette inherits the sidebar's behaviour exactly and no case gets worse, but a failed shared-view attach still strands the user on an empty workspace.

The workspace in the payload comes from `listed_herdr_agents` rather than from `herdr_row_workspace` (`app.rs:8943`). That helper answers `Option<WorkspaceKey>` over a doubly-nested option, where the outer `None` means "listed nowhere" — a state the palette can reach and the sidebar cannot. Carrying the workspace in the payload avoids the second lookup and the ambiguity with it.

The payload is at most one frame stale, since `palette_items` rebuilds every frame and activation happens in the frame that painted the row. A shared-view attach still shows its agent under "Herdr agents" for the few frames the gesture takes, because `claimed` is built from sessions and no session exists yet (`app.rs:8887`); a second Enter is deduped at `app.rs:1444`. That is the sidebar's behaviour too.

## 3. Sidebar filter rows

Three pieces move together: what the nav model emits, what the painter draws, and what the focus reconciler watches. Any one alone is a bug.

### The predicates split name from gate

Today `current_project_rows` fuses three dimensions into one bool per worktree (`app.rs:2787`):

```rust
let mut worktree = |_p: &Project, wt: &Worktree| {
    worktree_matches.get(&wt.path).copied().unwrap_or(false)
        && toggles_pass(&Some(wt.path.clone()))
        && worktree_pr_passes(any_pr, &pr_matches, &wt.path)
};
```

A child match cannot be OR-ed into that without also bypassing the toggle and PR dimensions. So `RowPredicates` splits it:

```rust
pub struct RowPredicates<'a> {
    /// Whether the workspace passes the toggle and PR dimensions.  A child
    /// match surfaces a workspace only through this gate, so a toggle keeps
    /// narrowing the tree while a query is live.
    pub gate: &'a dyn Fn(&WorkspaceKey) -> bool,
    /// Whether the workspace's own name matches the query.
    pub name: &'a mut dyn FnMut(&WorkspaceKey) -> bool,
    /// Whether a child row's display name matches.  `None` while the query is
    /// empty, which is what keeps a bare toggle from surfacing every child.
    pub child: Option<&'a mut dyn FnMut(&WorkspaceKey, &WorkspaceEntry) -> bool>,
    pub project_self: &'a dyn Fn(&Project) -> bool,
}
```

`child` is `None` unless the query is non-empty. This is not a nicety: `PanelFilter::matches` returns `true` for an empty query (`panel_filter.rs:159`) while `is_filtering` is true on toggles alone (`panel_filter.rs:104`), so a child predicate built from an empty query matches every child and every workspace with any child surfaces. The attention toggle would silently stop filtering.

The predicate needs the workspace alongside the entry because a `WorkspaceEntry::Agent` carries only its key (`sidebar_nav.rs:38`) and the display name lives in the endpoint caches. `current_project_rows` precomputes both name maps ahead of the closures for the same borrow reason the existing `worktree_matches` map is precomputed (`app.rs:2744`) — and with more force here, since session names need `session_herdr_agent` and `session_herdr_status`, which take `&self` and would collide with the matcher's `&mut self.project_filter`.

### Two rules

- A workspace matching **by its own name** shows all its children. That is today's behaviour and it stays.
- A workspace surfaced **only because a child matched** shows just the matching children.

So a workspace survives iff `gate(ws) && (name(ws) || any child matches)`, and `filtered_rows` emits all its children in the first case and the matching ones in the second. `push_session_rows` becomes that walk, emitting `Session` and `HerdrAgent` rows alike. The comment at `sidebar_nav.rs:333` and the two at `app.rs:4034` describe the old constraint and go with it.

Without the second rule, typing an agent's name hands back its workspace plus every sibling session, which is the opposite of searching.

### The painter intersects instead of stripping

Deleting the `retain` at `app.rs:4041` is necessary and not sufficient. The painter's visibility sets collect only `Home`, `Project` and `Worktree` rows out of the model (`app.rs:4008-4025`), and then draws every entry the *listing* holds under a surviving workspace (`app.rs:4047-4055`, painted at `:4300`). Under rule 2 the model emits one child and the painter would draw all of them, so the sibling rows would have no cursor path — the same stranding the `retain` exists to prevent, in the opposite direction.

The fix is mechanically parallel to what is already there: while filtering, collect `Session(id)` and `HerdrAgent(..)` from `rows` into their own sets and intersect `listed` against them, in place of the `session().is_some()` retain. The painter then draws exactly what the model listed, which is the invariant both halves have always been trying to hold.

### The reconciler learns about titles

`SessionInput` is `{workspace, id, attention}` (`sidebar_focus.rs:288`) and the per-frame comparison reads those three (`sidebar_focus.rs:490`). `reconcile_sidebar_focus` returns early when the inputs match (`app.rs:2897`) and the paint path reuses `sidebar_rows_cache` while filtering (`app.rs:3970`). Once titles enter the filter, a PTY title change — every prompt, in many shells — would neither add nor remove a row until something unrelated moved.

Agent names are already covered: `ObservedInputs` carries `herdr_generation` (`app.rs:2894`), and `rendered_differs` compares kind and title (`herdr.rs:770`).

`SessionInput` gains `title: &str`, and `sidebar_snapshot` stamps the empty string for it when `project_filter.query()` is empty. A title change then invalidates the snapshot only on frames where a query is live and the title can actually change the row set. Non-filtering frames cost exactly what they cost today; filtering frames rebuild the row list on every prompt, which is the price of the feature and is bounded by the sidebar being open with a query typed into it.

## Testing

`sidebar_nav.rs` is deliberately egui-free and already has the fixtures. The filter rules are unit tests there:

- An agent name match surfaces its workspace and only the matching agent.
- A workspace name match still brings every child, agents included.
- A session title match behaves identically to an agent name match.
- A herdr row survives a filter that matches its workspace, which is the regression test for the deleted `retain`.
- With `child` as `None`, a toggle-only filter emits exactly what it emits today. This is the one that fails if the empty-query gate is dropped.

`command_palette.rs` covers ranking and grouping without an `App`:

- `herdr` as a query ranks both an attached session row and an unattached agent row, proving the unpainted haystack entry.
- The new section groups after `OpenSessions` on an empty query.

Against the free function from section 1, beside the existing herdr tests in `app.rs`:

- An agent hidden by `show_unmatched = false` appears in no listing, so neither surface can show it.
- An agent an open session already holds is absent from the listing, which is what keeps it under Open sessions and out of Herdr agents.

## Open questions

1. Section 2's refusal fix touches both existing sidebar attach paths. It is a real bug and three lines, but it is not what this branch is for; splitting it into its own issue is defensible if review would rather keep the diff to the palette.
2. Nothing here surfaces a herdr *session* as distinct from its agents. herdr's `session list` is already read for the attach gesture (`herdr.rs:280`), so a "Herdr sessions" row set is reachable later; this spec deliberately stops at agents, which is what the sidebar models.
