# Sidebar filter actions, PR-state filters, and search scope

**Date:** 2026-07-29
**Status:** approved, ready for planning

## Goal

Make the sidebar's toggle filters first-class actions — bindable, palette-listed,
MCP-callable — add PR-state filters to the project panel, and let the user choose
whether a search query is confined by the active toggles. Getting the first of
those right requires hoisting an existing per-action guard into the input layer,
which is a prerequisite, not a side quest.

## Background — how filtering works today

Both sidebars own a `PanelFilter` (`panel_filter.rs`) with two independent
dimensions:

- **Toggle chars.** `PanelFilter::new(&['s', 'a'])` for the project panel
  (`app.rs:649`) and `&['m', 'd', 'u']` for the git panel (`app.rs:650`).
  A bare char in `Browsing` mode flips the toggle inside `on_text`
  (`panel_filter.rs:119-135`).
- **A fuzzy query**, entered with `/`, matched by nucleo in `matches`.

The two are ANDed. In `current_project_rows` (`app.rs:1749-1759`) a row must
satisfy both `toggles_pass` and the fuzzy match; in `filtered_git_rows`
(`app.rs:2094-2113`) both `kind_pass` and `query_pass`. Within a dimension the
project toggles AND each other while the git kind toggles union.

Filtering already force-expands collapsed projects (`sidebar_nav::filtered_rows`),
so toggles are the only thing narrowing a search today.

`[ui] pr_status = true` polls `gh` per worktree and paints a PR badge. The
lifecycle enum `PrState { Open, Draft, Merged, Closed }` already exists
(`pr_status.rs:29-35`). Lookups run on background threads that call
`ctx.request_repaint()` when they land (`pr_status.rs:132-140`), and are only
started for projects whose stored `expanded` flag is set (`app.rs:2895`).

Config is read once at startup. There is no reload path, so a `&'static`
toggle list chosen at construction never needs reconfiguring.

## Global constraints

- Every user-visible change is opt-in or reproduces today's behavior. There are
  exactly two deliberate exceptions — feature 0's "Consequence" section and
  feature 1's "Divergences from today's behavior" — and neither may grow.
- Only `alacritree/` and `docs/` are edited. The vendored `alacritty*` and
  `egui-winit` crates are read-only.
- No new bare-letter default keybinding may exist for a filter that does not
  exist today.
- Comments explain *why*, never restate *what*, and carry no reference to this
  spec, a PR, an issue, or a task.

---

## Feature 0 — hoist the search-mode guard into the input layer

This lands first. Features 1 and 3 add thirteen actions that would otherwise
each need a copy of the guard described below.

### The existing pattern and why it is in the wrong place

Four actions carry an identical guard at dispatch:

```rust
// app.rs:2356 (DeleteSelected); same at 2346, 2379, 2397
if origin != ActionOrigin::Ipc
    && self.project_filter.mode() != panel_filter::Mode::Browsing
{ return; }
```

It exists because egui emits both an `Event::Key` and an `Event::Text` for one
printable press (`egui-winit/src/lib.rs:790` then `:813`). While searching, the
text feeds the query and is consumed, but the key survives `drain_search_or_nav`
(`app.rs:4992-5010`) and reaches `handle_shortcuts`, which matches it because
`sidebar_focused` (`app.rs:1561`) does not consider mode. Each action then has to
defend itself.

`SidebarTop`, `SidebarBottom`, `SidebarNextProject` and `SidebarPreviousProject`
(`app.rs:2338-2345`) deliberately carry no guard: their default triggers are
Home/End/PageUp/PageDown, which emit no text, and navigating filtered results
mid-query is wanted behavior. That split is the tell — the rule is a property of
the *event*, not of the action:

> A key press whose text the search box consumed must not also run a binding.

The input layer knows this; the action does not. `origin != ActionOrigin::Ipc`
is an escape hatch that exists only because the rule sits in the wrong place —
IPC never enters the input layer at all.

### The rule

The invariant is exact and must be implemented exactly. Any rule that *guesses*
which keys produce text is wrong: an earlier draft of this spec used an escape
list of non-text keys and mis-classified `Shift`+letter, `Enter`, `Tab`,
`ArrowLeft` and `ArrowRight`.

**Pre-pass.** Inside the `ctx.input_mut` closure of `handle_sidebar_nav` and
`handle_git_sidebar_nav`, before the existing `i.events.retain(...)`, scan
`i.events` once and produce a `Vec<bool>` parallel to it: entry `n` is `true`
when `i.events[n]` is an `Event::Key { pressed: true, .. }` and `i.events[n + 1]`
is an `Event::Text`. `egui-winit` pushes the pair adjacently within a single
`on_keyboard_input` call (`egui-winit/src/lib.rs:790` then `:799`), so adjacency
is a reliable pairing.

**The result must be per-occurrence, not a `(Key, Modifiers)` set.** Two presses
in one frame can share a tuple — key conversion falls back to
`logical_key.or(physical_key)` (`egui-winit/src/lib.rs:764`) so distinct physical
keys can resolve to one `egui::Key`, and repeats reuse the tuple outright. If one
such press carries adjacent text and another does not, a value-keyed set consumes
both. `retain` visits every event exactly once in order, so the closure keeps an
index counter and reads the parallel `Vec<bool>` positionally.

**Consume.** `drain_search_or_nav` gains a sixth parameter, `produced_text: bool`,
supplied by the caller for that specific event. While the panel is in `Search`
mode and `produced_text` is set, the key is consumed and reaches no binding.
Everything else keeps today's behavior. Passing a `bool` rather than the set
keeps the function a pure decision over one event, which is how it is already
tested.

**Order inside `drain_search_or_nav`**, which is prescriptive:

1. **New.** `Search` mode and `produced_text` → consume.
2. Search-scoped binding dispatch (`app.rs:4978`), unchanged.
3. Modifier early-return (`app.rs:4992`), unchanged.
4. `filter.on_key` (`app.rs:4995`), unchanged.
5. Browsing nav / `Space` consume (`app.rs:5001-5009`), plus **new**: `Search`
   mode and bare `Delete` → consume as a no-op.

Step 1 must precede step 2. `SidebarSearchConfirm` and the two cancel actions are
ordinary configurable actions (`bindings.rs:190`), so a user can bind one to a
printable letter; with the check after the dispatch, that letter would run the
action instead of entering the query, contradicting the invariant. Text input is
unconditional — a key that types must type.

Step 1 must precede step 3, or `Shift`+letter escapes through the modifier
early-return. That is exactly what the escape-list draft missed, and it is why
the built-in `Shift+R` → `RenameSelected` is safe here.

The defaults are unaffected by putting step 1 first: `Enter` and `Esc` emit no
text (`\r` and `\u{1b}` are control characters, filtered at
`egui-winit/src/lib.rs:802`), so they fall through to step 2 as they do today.

**Bare Delete** is consumed at step 5, not step 1, so an explicit search binding
on it still wins. It emits no text, so the exact rule does not catch it, but as a
fallback it is a search-box editing key: the query is append-only with `Backspace`
popping the tail (`panel_filter.rs:102-105`), so there is no forward-delete to
perform and it must not reach the cursored row.

### Why this is exact

`egui-winit` suppresses `Event::Text` when `ctrl`/`command`/`mac_cmd` is held
(`egui-winit/src/lib.rs:807-810`) and for empty or control-character text
(`:802`). So the set naturally contains letters, digits, punctuation and
`Shift`+letter, and naturally excludes `Ctrl`+letter, `Enter`, `Tab`, `Esc`,
`Delete`, the arrows, `Home`/`End`/`PageUp`/`PageDown` and the function keys —
without a list to maintain against `egui::Key` gaining variants.

`filter.on_key` claims only `Backspace`, `ArrowUp` and `ArrowDown` in search mode
(`panel_filter.rs:101-113`); `ArrowLeft` and `ArrowRight` are *not* claimed and
must keep falling through, which the exact rule preserves.

### Palette origin

`run_palette_action` dispatches with `ActionOrigin::Keyboard` (`app.rs:6384`), so
the four guards currently also block palette invocation while the project filter
is searching. Deleting them would change that, which the keyboard-shaped rule
above does not cover.

Add `ActionOrigin::Palette`, used by `run_palette_action`, and one guard at the
top of `dispatch_action`:

```rust
if origin == ActionOrigin::Palette
    && action.requires_project_browsing()
    && self.project_filter.mode() != panel_filter::Mode::Browsing
{ return; }
```

`requires_project_browsing()` is a **new predicate matching exactly the four
guarded actions** — `RefreshProjects`, `DeleteSelected`, `RenameSelected`,
`ToggleProjectExpanded`. It must not reuse `is_sidebar_scoped`, which covers
eight (`bindings.rs:154-165`): `SidebarTop`, `SidebarBottom`,
`SidebarNextProject` and `SidebarPreviousProject` carry no guard today and run
from the palette during search, and a broad predicate would newly block them.

It must also exclude the eleven new filter actions and `RefreshPrStatus`. The
guard reads `project_filter.mode()`, which is meaningless for a git-panel action;
none of the four is a git action, and keeping the predicate to exactly those four
is what keeps it meaningful.

One site instead of four, and palette behavior is unchanged.

Note for a follow-up, out of scope here: the behavior being preserved is a
*silent* no-op — the palette row runs, nothing happens, and the user gets no
feedback. The guards were written for keyboard triggers; the palette hits them
only because it reuses `ActionOrigin::Keyboard`.

### Consequence

All four per-action guards are deleted, along with their `origin != Ipc`
exemptions. No new action needs one.

**Unchanged.** Bare letters typed while searching go to the query and fire
nothing. `Shift`+letter likewise — including the built-in `Shift+R` →
`RenameSelected` (`bindings.rs:462-466`), which today is stopped by
`RenameSelected`'s guard and now by the input rule. `Ctrl`+letter bindings keep
firing. `Home`/`End`/`PageUp`/`PageDown` keep navigating filtered results.
`ArrowLeft`/`ArrowRight`, `Tab`, and an unbound `Enter` keep falling through —
the last of these is asserted by an existing test
(`search_enter_with_no_binding_falls_through_without_activating`,
`app.rs:7689-7705`) that the escape-list draft would have broken. Bare `Delete`
stays inert. Palette dispatch is unchanged.

**Changed, and this is the point of the feature.** A text-producing key bound to
an action that carries *no* guard today stops firing mid-query: a letter bound to
an unguarded sidebar action (`SidebarTop` and its three siblings) or to any
unscoped action such as `Quit` fires during search on master and now types into
the query instead. That is the bug the four guards were patching one action at a
time — a key whose text the search box swallowed should not also run a command.

---

## Feature 1 — filter toggles become actions

### Rationale

Filter state is currently reachable only by typing a hardcoded char into a
focused panel. Modelling each toggle as a `NamedAction` puts it on the three
surfaces the codebase already has — `[[keyboard.bindings]]`, the command
palette, and `run_action` over IPC/MCP — instead of inventing a fourth.

`run_action` parses any action name with no allow-list (`app.rs:7153-7159`), and
`dispatch_action` does not re-check focus, so a new variant is MCP-callable the
moment it exists.

### New `NamedAction` variants

Each names its own panel, so a call from IPC/MCP is unambiguous regardless of
which pane has focus.

| Panel | Variants |
|---|---|
| Projects | `ToggleSessionsFilter`, `ToggleAttentionFilter`, `TogglePrOpenFilter`, `TogglePrDraftFilter`, `TogglePrMergedFilter`, `TogglePrClosedFilter`, `ClearProjectFilters` |
| Git | `ToggleModifiedFilter`, `ToggleDeletedFilter`, `ToggleUntrackedFilter`, `ClearGitFilters` |

The clear actions are not redundant with `Esc`: an agent cannot read toggle
state, so without them it has no way to reach a known-clear baseline.

Each variant needs an arm in `parse_action` (`bindings.rs:797`) and in
`description()` (`bindings.rs:221`). `config_name()` derives from `Debug`
(`bindings.rs:212-218`) and needs no change.

### Two new focus scopes

`handle_shortcuts` knows one sidebar predicate — `sidebar_focused =
self.focus == PaneFocus::ProjectsSidebar && !self.palette.is_open()`
(`app.rs:1561`) — which cannot express a git-panel key at all.

Add to `NamedAction`:

```rust
/// Valid only while the projects sidebar owns focus: their triggers are bare
/// letters that belong to the PTY anywhere else.
pub fn is_projects_filter_scoped(&self) -> bool { ... }

/// The git sidebar's equivalent.
pub fn is_git_filter_scoped(&self) -> bool { ... }
```

`handle_shortcuts` computes `git_focused` alongside `sidebar_focused` and
extends the per-action `valid_for_focus` filter (`app.rs:1579-1591`) so a
projects-filter-scoped action requires `sidebar_focused` and a
git-filter-scoped action requires `git_focused`.

These are pure focus predicates with no mode component. Feature 0 already stops
any text-producing key — bare letter or `Shift`+letter — from reaching
`handle_shortcuts` while that panel is searching, so a filter action bound to one
cannot fire mid-query whatever its trigger.

When no matched action survives `valid_for_focus`, `matched` is empty and the
event passes through untouched (`app.rs:1576`, `1603-1611`), so a bare letter
still reaches the PTY when the terminal has focus.

### Default bindings

`s`, `a` (projects) and `m`, `d`, `u` (git) become default `KeyBinding` entries
with `Modifiers::NONE` in `default_bindings()` (`bindings.rs:368`), reproducing
the chars `on_text` handles today.

The PR filters ship with **no default key**. Any bare letter would be new
behavior for every user, and the obvious mnemonics collide with common sidebar
bindings. They are reachable from the palette and by explicit binding.

### Collision semantics

`parse_bindings` drops any default whose `(key, mods)` matches a user binding
(`bindings.rs:357-360`), mirroring alacritty's override rule. Two *user*
bindings on the same trigger are **not** deduplicated against each other:
`all_matches` returns both and `valid_for_focus` routes them by panel. So a
context-sensitive double binding is expressible, and is how a user who has
claimed a letter recovers the default it displaced:

```toml
[[keyboard.bindings]]
key = "D"
action = "DeleteSelected"        # projects sidebar only

[[keyboard.bindings]]
key = "D"
action = "ToggleDeletedFilter"   # git sidebar only
```

`docs/alacritree.md` must document this, because the failure mode without it is
silent: a user with a pre-existing bare-letter binding loses the colliding
default toggle with no warning.

### Divergences from today's behavior

Toggling moves from text-driven to key-driven, and `egui-winit` resolves keys as
`logical_key.or(physical_key)` (`egui-winit/src/lib.rs:764`) so that non-Latin
layouts emit Latin-position keys. Two consequences, both accepted:

- On a non-Latin layout, the physical `S` position toggles the sessions filter
  even though the text it produces is not `"s"`. Today it does not toggle.
- With Caps Lock on, the text is `"S"` but the key event is `Key::S` with no
  Shift modifier, so the binding matches. Today uppercase text does not match
  the lowercase `allowed_toggles`, so it does not toggle.

Neither affects ordinary lowercase Latin input. Shift+letter remains inert
because bindings match modifiers exactly.

### `PanelFilter` changes

- The toggle branch of `on_text` (`panel_filter.rs:119-135`) is deleted.
  `Browsing` mode now recognizes only `/`; every other text event falls through
  unconsumed so the binding table can see the paired key.
- New `pub fn toggle(&mut self, key: char)` and `pub fn clear_toggles(&mut self)`,
  called by the action dispatcher. `toggle` ignores a char outside
  `allowed_toggles`.
- `allowed_toggles` stays `&'static [char]` and stops being a key list: it is now
  the ordered *identity* list that `toggle_bits` indexes and `active_toggles`
  renders. The `[s]` header chip keeps the single char as a stable label, not a
  key hint — rebinding the action does not change the chip.
- `Esc`-in-browsing clearing toggles (`panel_filter.rs:94-97`) is unchanged.

### Dispatch

`dispatch_action` gains arms that call `toggle`/`clear_toggles` on the named
panel's filter, with no mode guard (feature 0 covers it), and then mirror the
existing outcome handlers:

- Project filters: nothing further. `Outcome::FilterChanged` is deliberately a
  no-op (`app.rs:1665`) because the focus reconciler repairs the cursor later in
  the same `update` from a snapshot that still knows which row was hidden.
- Git filters: call `after_git_filter_changed()`, matching
  `apply_git_filter_outcome` (`app.rs:2039`).

### Palette

`bindable_actions()` (`command_palette.rs:218`) is a fixed-size array. Thirteen
actions are added across this spec — the eleven above, plus `ToggleSearchScope`
(feature 2) and `RefreshPrStatus` (feature 3) — so its type becomes
`[NamedAction; 63]`.

A new `PaletteSection::Filters` (title `"Filters"`) is added, and `section_of`
(`command_palette.rs:69-91`) routes the eleven filter actions plus
`ToggleSearchScope` to it. `RefreshPrStatus` files under `Sidebar`.

---

## Feature 2 — search scope

### Config and runtime state

```toml
[ui]
search_scope = "filtered"   # default, today's behavior
# search_scope = "all"
```

Parsed like the existing `sidebar_focus` key (`config.rs:343-353`): an
unrecognized string logs a warning and falls back to the default.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchScope {
    /// A query narrows the rows the active toggles already allow.
    Filtered,
    /// A query is evaluated against every row; the toggles stand aside until
    /// the query empties.
    All,
}
```

`AlacritreeApp` holds a `search_scope: SearchScope` seeded from config. It is
session-only and never written to `state.toml` — restarting returns to the
config value.

`NamedAction::ToggleSearchScope` flips it. It is **not** focus-scoped: it
affects both panels and has no default key, so there is nothing for a scope
predicate to protect.

### Rule

One helper, so the two panels cannot drift:

```rust
// PanelFilter
/// Whether the toggle filters apply this frame. Under `All` a live query
/// stands them down, so a search reaches rows the toggles hide.
pub fn toggles_apply(&self, scope: SearchScope) -> bool {
    scope == SearchScope::Filtered || self.query.is_empty()
}
```

**Project panel** (`current_project_rows`, `app.rs:1719-1769`): when
`toggles_apply` is false, both `toggle_sessions` and `toggle_attention` are
treated as `false`. Forcing only `toggles_pass` to return `true` is not enough —
`project_self` is `!any_toggle && project_matches...` (`app.rs:1754-1755`), so
leaving `any_toggle` set would keep project headers from matching their own name
during a wide search.

**Git panel** (`filtered_git_rows`, `app.rs:2094-2113`): when `toggles_apply` is
false, `m`/`d`/`u` are treated as `false`, which makes `any` false and
`kind_pass` accept every kind.

The toggles stay set throughout and resume the instant the query empties.

### Reconciler input

`sidebar_focus::UiInputs` carries `query` and `toggles` so the reconciler can
skip an unchanged frame (`app.rs:1796-1800`, `1834-1839`; compared at
`sidebar_focus.rs:388`). Flipping `search_scope` changes the effective row set
without changing either field, so `UiInputs` gains a `toggles_apply: bool` fed
from the helper. Both construction sites must pass it, or a scope flip leaves the
cursor unrepaired.

A `bool` keeps the comparison allocation-free. Every `UiInputs` literal needs the
new field — `app.rs:1796`, `app.rs:1835`, `sidebar_focus.rs:444`, and several in
`steady_state.rs:114`. Feature 3 adds `pr_generation` and `active_branch` to the
same struct and a branch to `ProjectInput`'s worktree tuple, so both features
touch the same sites and should land their field additions together.

### Indicator

`panel_header_filter_ui` (`app.rs:4934-4944`) paints active-toggle chips in
`theme.accent`. While `toggles_apply` is false it paints them in
`theme.text_muted` instead. The chips still report what is set; the color says
it is not applying right now.

---

## Feature 3 — PR-state filters

### Toggles

The project panel's `allowed_toggles` becomes `&['s', 'a', 'o', 'd', 'm', 'c']`
when `[ui] pr_status = true`, and stays `&['s', 'a']` when it is false — with
polling off the PR filters would hide every row, so they must not exist. Two
`&'static` slices selected at construction (`app.rs:649`); config is startup-only,
so no reconfiguration path is needed.

The four PR toggles form one dimension that **unions** internally (`o` + `d` →
open *or* draft) and **ANDs** with `s`/`a`, matching how the git panel unions its
kind toggles inside a dimension that ANDs with the query.

A pure predicate carries the rule:

```rust
// pr_status.rs
/// Whether a worktree in `state` survives the active PR toggles. No active
/// toggle passes everything; an unknown state (no lookup yet, no PR, `gh`
/// unavailable) never satisfies one.
pub fn pr_pass(state: Option<PrState>, open: bool, draft: bool, merged: bool, closed: bool) -> bool
```

### Reading cached state

`current_project_rows` has no `&egui::Context` and so cannot call
`PrCache::poll`. It reads a new non-polling accessor:

```rust
// PrCache
/// The state of a cached lookup, without starting or refreshing one.
/// `None` unless the entry was queried for `branch` — a cache entry is keyed
/// by path but only valid for the branch it was looked up against.
pub fn state(&self, path: &Path, branch: Option<&str>) -> Option<PrState>
```

The branch argument is required, not optional polish. `poll` treats identity as
`(path, branch)` and clears `info` on a mismatch (`pr_status.rs:80`, `:106`), and
the project loop deliberately picks either the live status branch or `wt.branch`
(`app.rs:2909-2917`). A path-only accessor would hand the filter the *previous*
branch's PR state in the window before paint-time `poll` invalidates it, which
would both filter wrongly and drive a cursor repair off bad rows. Callers pass
the same branch snapshot the poll loop uses.

Because a finished lookup calls `ctx.request_repaint()` (`pr_status.rs:137`),
rows appear as `gh` answers land rather than waiting for a keystroke.

### Keeping filtered rows fresh

`ObservedInputs` observes projects, sessions, `session_rows_always`, `query` and
`toggles` (`sidebar_focus.rs:341-347`) — and nothing else. Once
`current_project_rows` depends on `PrCache::state`, a landed lookup changes the
row set without changing any observed input, so `reconcile_sidebar_focus` takes
its unchanged-frame early return (`app.rs:1841-1843`) and painting reuses
`sidebar_rows_cache` (`app.rs:2778-2781`). Under an active PR toggle, newly
matching rows would never appear.

`PrCache` gains a `generation: u64`, bumped whenever `drain_completed` banks a
result and whenever a refresh invalidates entries. `UiInputs` and
`ObservedInputs` gain `pr_generation: u64`. A `u64` compare keeps the
steady-state path allocation-free.

**Feed it `0` unless a PR toggle is active.** `pr_generation` is only meaningful
while PR state can affect row membership. Passed unconditionally, every banked
result would invalidate the reconciler for every user — with `pr_status = true`,
a cap of `1` and thirty worktrees, that is roughly thirty full row rebuilds on a
cold cache for someone who never touches a PR filter. Flipping a PR toggle
changes `toggles`, which forces a rebuild on its own, so nothing is lost by
zeroing the generation when none is set.

**Branch changes must be observed too.** The `(path, branch)` identity that makes
`PrCache::state` safe also makes it *change* when the branch changes: a checkout
flips a worktree's cached state from `Some` to `None` with no lookup completing,
so `generation` does not move. Nothing else observes it either — `ProjectInput`'s
worktree tuple is `(path, name, prunable)` (`sidebar_focus.rs:331-336`) and no
observed input reads `git_status.current_branch()`. Without this, a row that
matched under the old branch stays visible until the replacement lookup banks.

The effective branch for a worktree is what `app.rs:2909-2917` computes today:

```rust
let is_active = current_workspace.as_deref() == Some(&wt.path);
if is_active {
    git_status.get(&wt.path).and_then(|c| c.current_branch()).or(wt.branch.as_deref())
} else {
    wt.branch.as_deref()
}
```

Extract it so the reader and the poll loop cannot drift, taking the live branch
already resolved rather than reaching for `git_status` itself:

```rust
/// The branch a worktree's PR lookup is keyed to. The active worktree prefers
/// its live status branch; every other worktree, and an active one whose
/// `StatusCache` has not produced a branch yet, uses the stored snapshot.
fn effective_branch<'a>(
    wt: &'a Worktree,
    current_workspace: Option<&Path>,
    live_branch: Option<&'a str>,
) -> Option<&'a str>
```

The `.or(wt.branch)` fallback is load-bearing, not decoration: a workspace that
has just become active has a freshly created `StatusCache` whose first result has
not landed, so `current_branch()` is `None` and the poll loop falls back to the
stored branch. A `state` caller that dropped the fallback would pass `None` while
the poll loop passed `Some(branch)`, hiding a row whose cached PR state is
perfectly valid.

Three inputs determine it, and **all three** must be observed:

- `ProjectInput`'s worktree tuple gains `wt.branch`, alongside the name it
  already stores.
- `UiInputs` gains `active_branch: Option<&'a str>`, the live
  `git_status.current_branch()` for the current workspace — the source that moves
  within ~1.5 s of an in-terminal checkout, well before `refresh_project` updates
  `wt.branch`.
- `UiInputs` gains `active_workspace: Option<&'a Path>`.

The third is not redundant. `active_branch` alone is a scalar with no record of
which worktree it applies to, so a workspace switch can leave it unchanged while
every effective branch moves: worktree A active with stored `main` and live
`feature`, worktree B inactive with stored `develop` and a dormant cache also on
`feature`. Switching A→B changes only `current_workspace` (`app.rs:973`), so
`active_branch` stays `Some("feature")` and no worktree tuple moves — yet A's
lookup key goes `(A, feature)` → `(A, main)` and B's goes `(B, develop)` →
`(B, feature)`. The session list does not save this either: both worktrees having
live sessions leaves it and its order identical.

Covered by these three, with `refresh_project` installing new records through
`poll_project_refreshes` (`app.rs:782`, `:808`) and worktree create/delete routing
back through it (`app.rs:6464`, `:6488`): stored-branch changes, worktree
addition and removal, IPC-triggered refresh, in-terminal checkout, and workspace
switching.

### Widening the poll

Today only worktrees of `expanded` projects are polled (`app.rs:2895`), and that
reads the *stored* flag, not the filtered projection. A collapsed project would
therefore never acquire PR data and could never match a PR toggle, which reads as
broken rather than empty.

While any PR toggle is active, the poll loop covers every worktree of every
project regardless of `expanded`. When no PR toggle is active the loop is
unchanged.

**One poll per path, not per row.** `PrCache` is keyed by path alone
(`pr_status.rs:46`) and clears `info` whenever an incoming branch disagrees with
the cached one (`pr_status.rs:106`), so it structurally cannot hold two branches
for one path. The same path can appear as a worktree of two projects — add a repo
and one of its own worktrees as separate sidebar projects and it does — and their
stored `wt.branch` snapshots can disagree transiently, since each is captured at
its own project's last discovery. Two callers alternately invalidating each
other's lookup would burn a `gh` process per frame for as long as the snapshots
disagree.

The widened loop therefore iterates a path-deduplicated set: first occurrence in
project order wins, is polled once, and its result is reused for every row on
that path. `PrCache::state` readers resolve the same way, so the reader and the
poller can never disagree about which branch a path was queried for. This is
honest about the cache's shape rather than papering over it — one path, one
branch, one lookup.

Consequence, accepted: the reconciler observes every duplicate row's stored
branch, so changing a *losing* duplicate's snapshot forces a rebuild that cannot
change what is displayed. Over-observing costs one needless rebuild in a
degenerate configuration; under-observing would cost correctness, and narrowing
the observation to winning occurrences would put the dedup rule in two places.

The loop lives inside `show_project_sidebar` (`app.rs:2887`), which runs only
when the left sidebar is visible (`app.rs:7276`). Widened polling is therefore
conditional on that panel being painted: a PR toggle activated over IPC or from
the palette while the sidebar is hidden starts no lookups until it is shown.
This is accepted rather than hoisting the loop — the panel being filtered is the
one that is hidden, so there is nothing to narrow in the meantime, and the first
paint after it reopens starts the lookups. The rows fill in as results land
rather than appearing all at once, which `docs/alacritree.md` must say.

### Concurrency cap

```toml
[ui]
pr_status_concurrency = 0   # default: unlimited, exactly today's behavior
```

`PrCache::poll` spawns one thread per path (`pr_status.rs:132-140`) and the paint
loop polls every eligible worktree in a single frame, so a cold cache already
forks one `gh` process per worktree. Widening raises that count.

A non-zero value caps concurrent lookups. Implementation is a counter, not a
queue: `poll` skips spawning when `in_flight >= cap` and the next frame retries.
Progress depends on every lookup waking the app, which today is not guaranteed:
`spawn_lookup` calls `ctx.request_repaint()` only after `query_gh` and `send`
(`pr_status.rs:132-140`), so a worker that panics disconnects its receiver
without repainting. On an idle app with a full cap, no further frame runs,
`drain_completed` never observes the disconnect, and polling stalls permanently.
The repaint must therefore move into a guard whose `Drop` fires on every thread
exit, panicking or not. Decrementing on `TryRecvError::Disconnected` is necessary
but only useful once something wakes the app to observe it.

Default `0` is required by the global constraint: any positive default would
change how fast PR badges populate on an existing install.

### Draining completions

The inline drain at the head of `poll` (`pr_status.rs:88-96`) cannot own the
counter's decrement, because an entry whose project collapses mid-lookup is never
polled again and would strand a slot forever. It is replaced by:

```rust
// PrCache
/// Sweep every entry's pending receiver, banking results and freeing
/// concurrency slots for lookups whose caller has stopped polling them.
pub fn drain_completed(&mut self)
```

Three requirements:

- It decrements `in_flight` on `TryRecvError::Disconnected` as well as on a
  received result. A worker that panics or exits without sending would otherwise
  leak a slot permanently, stalling all polling once the cap is reached.
- It runs **once per frame in `update`, before any painting**, unconditionally.
  There are two poll sites — the project panel (`app.rs:2895`) and the git
  sidebar (`app.rs:3530`) — and either sidebar may be hidden, so hanging the
  drain off a panel would strand entries whenever that panel is not drawn.
- It honors the refresh flag below rather than unconditionally stamping
  `queried_at`.

### Refresh action

`NamedAction::RefreshPrStatus` invalidates every cache entry so the next paint
re-polls. Clearing `queried_at` alone is not sufficient: `poll` only spawns when
`pending.is_none()` (`pr_status.rs:109`), so an entry with a lookup already in
flight would not be re-queried, and the drain would then stamp a fresh
`queried_at` and swallow the request.

`Entry` gains `refresh_requested: bool`. The action clears `queried_at` on every
entry, and sets `refresh_requested` **only where `pending.is_some()`**.
`drain_completed` banks a result for a flagged entry but leaves `queried_at` as
`None` and clears the flag, so the following poll starts a fresh lookup.

The action must also call `ctx.request_repaint()`. Both poll sites run during
sidebar painting (`app.rs:2895`, `app.rs:3530`), but the palette dispatches after
both sidebars have painted (`app.rs:7380`), so a palette-origin refresh
invalidates the cache too late for the current frame. Bumping `generation` and
rebuilding in the second reconcile pass does not schedule a frame by itself, and
"the next paint re-polls" must be an invariant rather than a bet on incidental
repaints. The same applies to an IPC-origin refresh, so the repaint is
unconditional rather than origin-dependent.

Setting the flag unconditionally would double-poll every idle entry: with
`pending` empty there is nothing for the drain to bank, `poll` starts the
requested lookup because `queried_at` is `None`, and the drain then honors the
still-set flag by refusing to stamp `queried_at` — so the next poll starts a
second, redundant lookup. The flag is only meaningful for a request that arrives
while a lookup is already in flight, which is the one case `queried_at` alone
cannot express.

It is **sidebar-scoped** (`is_sidebar_scoped`), joining `RefreshProjects` and
`DeleteSelected` — a bare letter bound to a global action would consume that
letter as terminal input, because `handle_shortcuts` suppresses the paired text
event for any matched key (`app.rs:1604-1611`). It ships with no default key.

`is_sidebar_scoped` means the *projects* sidebar specifically — `sidebar_focused`
is `focus == PaneFocus::ProjectsSidebar` (`app.rs:1561`, `1580`). So a keyboard
binding for `RefreshPrStatus` does not fire while the git sidebar owns focus,
even though PR data feeds that panel's base-branch choice (`app.rs:3530-3531`).
That is accepted rather than adding an either-sidebar scope; the palette and
`run_action` reach it from anywhere.

---

## Config surface (all new keys)

```toml
[ui]
pr_status = true              # existing, gates the PR filter toggles
search_scope = "filtered"     # new: "filtered" | "all"
pr_status_concurrency = 0     # new: 0 = unlimited
```

No new `[ui]` table. Key remapping is `[[keyboard.bindings]]`.

`docs/alacritree.md` documents `search_scope`, `pr_status_concurrency`, the
thirteen new action names, the fact that the project panel's PR toggles have no
default key, and the collision rule with its two-binding recovery pattern.

## Testing

No `AlacritreeApp` test harness exists — `Session` owns a real PTY — so the
codebase's pattern is pure functions over plain data, tested in place.

**Feature 0 (TDD, RED first).** The decision lives in `drain_search_or_nav`, a
free function over `(steps, filter, bindings, key, modifiers)` already unit-tested
directly (`app.rs:7588` onward); the new text-key set becomes a sixth parameter,
so every existing call site in those tests needs updating.

Assert, in `Search` mode, with `produced_text` set: a bare letter is consumed and
reaches no binding; `Shift`+letter is consumed (this is the `Shift+R` →
`RenameSelected` case); a letter **bound to `SidebarSearchConfirm`** is still
consumed as query input rather than dispatched, which is the assertion that pins
step 1 ahead of the search-scoped dispatch. With `produced_text` clear:
`ArrowLeft`/`ArrowRight` are retained, `Tab` is retained, `Ctrl`+letter is
retained, `Home`/`End`/`PageUp`/`PageDown` are retained. Independent of the set:
bare `Delete` is consumed, bare `Space` stays consumed, and
`search_enter_with_no_binding_falls_through_without_activating`
(`app.rs:7689-7705`) still passes unchanged. In `Browsing` mode nothing is
consumed by the new rule, so letters still reach the binding table.

The pre-pass itself is a pure function over `&[egui::Event]` returning a
`Vec<bool>`, tested separately: an adjacent `Key`+`Text` pair marks the key's
index; a `Key` with no following `Text` does not; a `Text` with no preceding
`Key` marks nothing; a released key (`pressed: false`) is not marked. **And the
aliasing case:** a frame holding two pressed `Event::Key`s with the same
`(key, modifiers)` where only the first is followed by `Text` marks index 0 and
not the second — the test that fails against any value-keyed set and so pins the
per-occurrence representation.

Confirm the bare-letter and `Shift`+letter cases are RED against master before
the rule lands. Then assert the four dispatch arms no longer inspect mode; that
`requires_project_browsing()` is exactly those four actions and excludes
`SidebarTop`, `SidebarBottom`, `SidebarNextProject`, `SidebarPreviousProject`,
the eleven filter actions and `RefreshPrStatus`; and that a `Palette`-origin
invocation of one of the four is refused while the project filter is searching
while a `Palette`-origin `SidebarTop` still runs.

**`panel_filter.rs`**
- `toggles_apply` over both scopes × empty and non-empty query.
- `on_text` no longer consumes a toggle char in `Browsing`; `/` still enters
  search; search-mode text still appends.
- `toggle` flips a char in `allowed_toggles` and ignores one outside it;
  `clear_toggles` empties the set; `toggle_bits` and `active_toggles` still
  report `allowed_toggles` order.

**`pr_status.rs`**
- `pr_pass`: no active toggle passes every state including `None`; a single
  toggle passes only its state; two toggles union; `None` never satisfies an
  active toggle.
- `state` returns the cached state for the branch it was queried against and
  `None` for any other branch, including `None`.
- The effective-branch expression returns the live status branch when the
  workspace is active and its cache has one, falls back to `wt.branch` when the
  active workspace's cache has no branch yet, and uses `wt.branch` for every
  inactive worktree. Extracting it as a free function over
  `(worktree, current_workspace, live_branch)` is what makes it testable and what
  keeps the reader and the poll loop from drifting apart.
- A path listed as a worktree of two projects with disagreeing stored branches is
  polled once, for the first occurrence in project order, and both rows read that
  same result.
- `drain_completed` banks a result and decrements the in-flight count for an
  entry that is never polled again, and decrements on a disconnected receiver.
- A refresh request survives an in-flight lookup: with `refresh_requested` set,
  draining a completed lookup leaves `queried_at` as `None` so the next poll
  re-queries. A refresh on an idle entry polls exactly once, not twice.
- `generation` advances when a result is banked and when a refresh invalidates,
  and is stable across a frame that banks nothing.

**`pr_status.rs`, orchestration.** The primitives above are not enough on their
own; these pin the behavior that makes them correct.
- With `pr_status_concurrency = 2` and four stale paths, `poll` spawns two
  lookups and skips the rest; after one completes and is drained, the next poll
  spawns exactly one more. With `0`, all four spawn.
- The repaint guard fires on a worker that panics before sending, not only on a
  worker that returns. A counter decrement without a wake does not prove the app
  ever runs another frame to observe it.
- `effective_branch` returns the live branch for the active worktree, falls back
  to the stored branch when the active worktree's live branch is `None`, and
  returns the stored branch for an inactive worktree — including the case where
  the inactive worktree's own cache holds a different branch.

**Row projection, not just the helper.** `toggles_apply` being right does not
prove the panels use it. Assert over the row sets themselves:
- Under `SearchScope::All` with a live query, a project row whose workspace has
  no sessions survives while `s` is toggled, a worktree with no PR survives while
  a PR toggle is set, and a project header matches its own name — the
  `any_toggle` case from feature 2.
- Under `SearchScope::All` with a live query, a git row of a kind no active
  toggle admits survives.
- Under `SearchScope::Filtered`, all of the above are excluded, and clearing the
  query restores exclusion under both scopes.
- Widened polling covers a collapsed project's worktrees only while a PR toggle
  is active; with none active the expanded-only set is unchanged.
- `pr_generation` is fed as `0` when no PR toggle is active and as the cache's
  value when one is.

**Dispatch.** Each of the eleven filter actions flips the toggle on its own
panel and not the other; a git filter action runs `after_git_filter_changed`
while a project one does not; `ToggleSearchScope` flips the runtime scope;
`RefreshPrStatus` invalidates the cache and requests a repaint; and
`drain_completed` runs once per `update` ahead of both poll sites.

**`sidebar_focus.rs`**
- `ObservedInputs::matches` reports changed when only `pr_generation` differs,
  when only `toggles_apply` differs, when only a worktree's branch differs, when
  only `active_branch` differs, and when only `active_workspace` differs — the
  last with `active_branch` held constant, which is the workspace-switch case a
  scalar branch cannot see. Each is a guard against a silently stale row set.
- `matches` still reports unchanged for an otherwise identical frame, and the
  `steady_state.rs` allocation assertion still holds with the four added fields.

**`bindings.rs`**
- Every new action round-trips `parse_action` ↔ `config_name` and has a
  non-empty `description`.
- The seven projects-filter actions are `is_projects_filter_scoped` and not
  `is_git_filter_scoped`, and vice versa for the four git ones; neither set is
  `is_sidebar_scoped`, `is_search_scoped`, or `is_palette_scoped`.
- `RefreshPrStatus` is `is_sidebar_scoped`; `ToggleSearchScope` is unscoped.
- The default bindings table contains bare `s`/`a`/`m`/`d`/`u` mapped to the five
  pre-existing filters, and no default binding for any PR filter.
- Two user bindings on one trigger both survive `parse_bindings` and both come
  back from `all_matches`, while a default on that trigger is dropped.

**`command_palette.rs`**
- `action_items` lists all thirteen new actions; the eleven filter actions and
  `ToggleSearchScope` file under `PaletteSection::Filters`.

**`config.rs`**
- `search_scope` parses both values, defaults to `Filtered`, and falls back with
  a warning on garbage.
- `pr_status_concurrency` defaults to `0`.

## Out of scope

- `nvim`-style multi-key toggle sequences (`g m`). The single-char namespace is
  nowhere near exhausted, and a prefix needs a pending-key state machine with its
  own abort and timeout semantics.
- Persisting `search_scope` to `state.toml`.
- Per-panel `search_scope`. One global flag was chosen.
- Making the header chip show the currently bound key rather than the identity
  char.
- Forward-delete inside the search query. Bare Delete is inert by design; the
  query is append-only.
- Searching session row labels. The filter matches workspaces, not sessions
  (`sidebar_nav::filtered_rows`), and that is unchanged here.

## Unresolved questions

None. All decisions above are settled.
