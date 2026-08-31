# Sidebar focus preservation

Date: 2026-07-23
Status: approved, ready to plan

Supersedes the instrumented-call-site draft of the same date. That version
hooked every mutation and navigation site; the site list was unbounded and
review found several missing. This version derives the same behavior by
observation.

Revised twice.

The first revision followed review of the first plan. Observation turned out to
be blind to three things it needs to see — a cursor reset that runs *before* the
reconciler, a navigation that changes no observed value, and a model whose only
accessor is the projection. Each is now handled explicitly rather than inferred.

The second dropped the `"reset"` mode. What was an opt-in feature with an
untouched default became default behavior with no off-switch, which changes the
standard the design is held to rather than just its default value: the frame
cost is now a tax nobody can decline, the filter path has no fallback beneath
the reconciler, and no configuration exercises the pre-deferral code any more.
Those are handled in *One config key, and the parity exception it takes*,
*Enforcing the cost budget*, and *The three unavoidable deferrals*.

## Problem

The projects sidebar keeps a keyboard cursor (`sidebar_cursor: Option<SidebarRow>`)
separate from what the terminal shows (`current_workspace` + `active_session`).
When the cursor's row stops being rendered, `ensure_cursor` (`sidebar_nav.rs:179`)
falls back to `rows.first()` — in practice always Home. Two everyday actions hit
that fallback:

1. **Filtering.** A `/` query or an `s`/`a` toggle hides the cursor's row. The
   cursor drops to Home even though the row still exists and returns the moment
   the query widens.
2. **Deleting.** Closing a session or removing a worktree leaves the cursor
   pointing at a row that no longer exists, so it resets to Home. A delete in the
   middle of a project throws the user to the top of the tree.

Both feel like being ejected from where you were working.

## Behavior

### Filter and delete get different rules

A filtered-out row still exists; only the projection changed. A deleted row is
gone; the model changed. That sets the direction of the repair:

| Trigger | Rule | Direction |
| --- | --- | --- |
| Filter hides the cursor's row | nearest surviving ancestor (session → worktree → project → home) | up |
| Cursor's row leaves the model | the row that slid into the vacated slot, bounded to the removed row's parent | sideways |

Worked against the reference tree:

```
- home
- project1
  - worktree1
    - session1   <- (1) lands here
    - session2   <- (1) deleted
  - worktree2
    - session1
    - session2   <- (2) deleted
    - session3   <- (2) lands here
- project2
  - worktree1
    - session1
    - session2
  - worktree2    <- (3) deleted
  - worktree3    <- (3) lands here
```

- (1) last child: the vacated slot holds `worktree2`, outside the parent, so it
  falls back to the previous sibling `session1`.
- (2) middle child: the slot holds `session3`, inside the parent — take it.
- (3) middle child one level up: the slot holds `worktree3`, inside `project2`.
- A worktree's only session: no sibling either way, so the cursor lands on the
  worktree row itself.

### The terminal follows deletes only, never filters

An opt-in makes the terminal follow a delete-induced landing. It does not follow
filter-induced landings:

- **Path dependence.** Filter shifts happen per keystroke. The *first* character
  that excludes the current row would decide where the terminal goes, even if
  later characters narrow to something better: `/al` jumps, `/alacritree`
  arrives too late.
- **Cancellation would commit a side effect.** `sidebar_search_cancel`
  (`app.rs:2146`) reseeds the cursor from `current_workspace`. If filtering had
  already moved the terminal, cancelling would faithfully reseed to the wrong —
  new — workspace.
- **Confirm is deliberately two-step.** `sidebar_search_confirm` (`app.rs:2088`)
  lands the cursor without activating; a following browsing `Enter` activates.
- **The `s`/`a` toggles are not exempt.** They flip in one keypress rather than
  incrementally, but they are still reversible visibility predicates, not
  activation commands. `a` in particular tracks live state that changes with no
  user input at all.

### Latent filter anchor

Climbing to an ancestor is lossy on its own: widening the query does not bring
the cursor back down, so a fruitless search still costs the user their place. A
hidden cursor is therefore remembered, and the cursor returns to it when the row
becomes visible again.

**The anchor lives and dies with one filter episode.** It exists only because a
filter hid a row, so it is dropped the moment nothing is filtering — when the
query is empty and no toggle is set. All three search exits go through
`PanelFilter::exit_search` (`panel_filter.rs:146`), which clears the query, so
confirm (`app.rs:2088`), cancel (`app.rs:2146`), and Shift+Esc
(`app.rs:2169`) all end the episode without any of them being edited.

This matters because those three land the cursor deliberately: confirm keeps the
row the user picked, cancel reseeds from the terminal. Without episode scoping a
stale anchor could later yank the cursor off a confirmed row when the filter
widened — and in the case where the confirmed row *is* the row the climb already
landed on, nothing observable happened at all for a value-comparison check to
catch.

### One config key, and the parity exception it takes

```toml
[ui]
sidebar_focus = "preserve"   # default: ancestor climb + sibling slide + latent anchor
# "follow"                   # preserve, plus the terminal follows delete landings
```

Two values, not three. An earlier draft kept `"reset"` as a default that
short-circuited the reconciler and left every existing path untouched. It was
removed deliberately, and what that buys and costs should both be on the record.

**What it buys.** `preserves()` stops existing — with two variants it is `true`
for both, a predicate that never discriminates while reading at each call site
as if it did. `after_filter_changed` stops having two behaviors to keep working
and is deleted rather than guarded. The projection cache is always live, so an
unchanged filtering frame gets *cheaper* than it is today instead of only
sometimes. Three permanent forks in the code, gone.

**What it costs, stated plainly.** `CLAUDE.local.md` requires new UX behavior to
sit behind a config option so the existing workflow is unaffected. This takes an
exception to that rule: `"preserve"` is the default, so upgrading changes where
the cursor goes with no config edit, and neither value restores the old
drop-to-first-row behavior. Calling the old behavior a bug is a reason to change
it, not a reason the compatibility rule does not apply — a user may have adapted
to reset-on-filter precisely because it is predictable. There is no in-version
rollback; the fallback is an older release.

This is a deliberate decision by the repo owner, taken with the alternative
(keep a third `"legacy"` value) on the table and rejected because retaining it
would have preserved every fork listed above and delivered none of the
simplification. It is recorded here rather than left for a reviewer to infer
from the diff, and the PR description says the same thing.

Two consequences follow and are handled as first-class design constraints rather
than fallout:

1. **The reconciler has no off-switch**, so its steady-state cost is an
   invariant every user pays on every frame. See *Enforcing the cost budget*.
2. **The deferrals are no longer covered by a parity mode.** With `"reset"`
   gone, no configuration exercises the old immediate paths in
   `after_filter_changed`, `close_session`, or `run_pending_delete`. Nothing in
   the suite can compare deferred behavior against them, so a named manual pass
   does that instead.

`"follow"` being opt-in gates only the terminal navigation. It does not make the
feature as a whole config-gated, and should not be described as if it does.

## Prerequisite: discovery must report failure

`Project::discover_wsl` (`projects.rs:75`) degrades to `Project::placeholder`
(`projects.rs:56`) on *any* `wsl::run_batch` error, and a placeholder holds
exactly one pseudo-worktree pointing at the root. `Project::refresh`
(`projects.rs:181`) and `poll_project_refreshes` (`app.rs:768`) then copy that
worktree list over the real one unconditionally.

So a transient `wsl.exe` hiccup is byte-identical to "every worktree in this
project was deleted". This is a live bug today — a flaky WSL call silently
collapses the sidebar — and it is disqualifying for a design that infers removal
from absence: the reconciler would fire a bogus slide and, under `"follow"`,
navigate the terminal. A debounce cannot help, because one failed result stays
installed until the next refresh.

Fix it at the source. Discovery reports whether its answer is authoritative:

```rust
pub struct Discovered {
    pub project: Project,
    /// False when the backend could not be reached and `project` is a
    /// placeholder standing in for an unknown tree. A caller that already has
    /// a worktree list must keep it rather than adopt the placeholder.
    pub authoritative: bool,
}
```

- A real repository, and a Windows root that genuinely is not a repository,
  are both authoritative — the empty/pseudo tree is the truth.
- A failed WSL round trip is not.

`Project::refresh` and `poll_project_refreshes` keep their existing `worktrees`
and `default_branch` when the answer is not authoritative. First-time discovery
has nothing to preserve and still shows the placeholder, exactly as today.

**This lands as its own branch and its own PR, off `master`, before the
reconciler** — planned separately in
`plans/2026-07-23-wsl-discovery-authoritative.md`. It is a live bug independent
of any sidebar work, so it should be reviewable without the reconciler attached,
and it fixes the same failure shape as `17f95c23` (a transport error collapsing
into a definitive negative answer in the foreground-TUI probe cache), which also
shipped alone.

## Architecture: one reconciler

Rather than every mutation site reporting what it did, one reconciler observes
what changed. This is what keeps the removal-site list from existing at all: an
unrelated deletion leaves the cursor present, so the reconciler returns it
unchanged with no guard to write.

### Three concepts, not two

Absence from `visible_rows` (`sidebar_nav.rs:42`) does **not** mean removal. It
omits worktrees under collapsed projects, and `listed_session_ids`
(`app.rs:5089`) drops session rows when the listing threshold stops being met.
Both are projection changes. The reconciler needs:

- **Model membership** — every project, every worktree regardless of expansion,
  every live session regardless of listing rule. Absence here means removal.
- **Current projection** — exactly the rows currently navigable, after
  expansion, listing rules, and filters. Absence here with presence in the model
  means the filter hid it.
- **Previous projection** — last reconcile's row order, which still holds the
  parent and sibling relationships a slide needs after they are gone from the
  model.

**Model membership has no existing accessor, and the nearest one is a trap.**
`listed_session_ids` (`app.rs:5089`) is *the projection*: it runs
`sidebar_session_ids` (`app.rs:4488`), whose threshold is `if always { 1 } else
{ 2 }`, and then inserts only non-empty lists. Build the arena from it and a
worktree that drops from two sessions to one reports **the surviving session** as
removed — the reconciler would slide off a session that is still running. The
same map also omits sessions whose project was dropped by `remove_project`
(`app.rs:1221`), which deliberately keeps those sessions alive.

The arena is therefore built from the live `(workspace, id)` pairs directly, and
`ListedSessions` supplies only the `projected` flag. Sessions whose workspace has
no sidebar row — orphans of `remove_project` — stay in the arena as unprojected,
parentless model nodes. They are unreachable by the cursor, which is the point:
present in the model, so never mistaken for deleted.

### Snapshot

```rust
struct TreeSnapshot {
    /// Model membership: stable SidebarRow key + parent NodeId.
    nodes: Vec<Node>,
    /// Current projection, in render order.
    projected: Vec<NodeId>,
    inputs: ObservedInputs,
}
```

`NodeId` is snapshot-local. Cross-snapshot matching still goes through the
stable path/session key, preserving the property `sidebar_nav.rs:1-5` exists to
protect: the project list mutates under the cursor, so an index would silently
retarget.

### Change detection

`ObservedInputs` is compared **borrowed**, allocating nothing: ordered project
roots, names, expansion flags, and worktree lists; session ids, workspaces, and
attention states; listing mode; query string; toggle bits.

"Allocating nothing" rules out `PanelFilter::active_toggles`
(`panel_filter.rs:70`), which collects a `Vec<char>` on every call. The toggle
set is captured as one `is_toggled` bool per entry in `allowed_toggles` — the
same two values, read without touching the heap.

- **Unchanged** — stop. Steady-state cost is one
  `O(projects + worktrees + sessions)` comparison, zero heap allocation, no
  `visible_rows` walk and no fuzzy matching.
- **Changed** — rebuild the arena and projection once. One owned path per model
  node, `usize` projection entries.

### Cost budget

This runs on every frame of a terminal emulator, for every user, with no
setting that turns it off. That last clause is what makes this section binding:
the steady-state cost is not a target to aim at but a tax nobody can decline.
The whole reconciler is tree walking over a few dozen rows, so it belongs in
microseconds; anything quadratic or allocating in the steady state is a defect,
not a tuning opportunity.

**Steady state (every frame, nothing changed):** one `ObservedInputs` compare —
a linear scan of contiguous memory, zero allocation, early-exit on the first
difference. Nothing else runs.

**Rebuild (a keystroke, a delete, a refresh):** `O(projects + worktrees +
sessions)` and one pass of the fuzzy matcher, which is what the sidebar already
pays on such a frame. Specifically forbidden:

- `rows.contains(row)` per node while building the arena. That is n² `PathBuf`
  comparisons. The arena is pushed in exactly the order `visible_rows`
  (`sidebar_nav.rs:42`) emits, with unprojected nodes interleaved, so a single
  monotonic index into `rows` decides `projected` for every node in one pass —
  no hashing, no set, better locality than either.
- Any `Vec::contains` used as set membership inside a per-node loop.

`find`, `climb`, `slide`, and `children` stay linear scans. They run a handful
of times per *repair*, not per node, and at these sizes a scan over a contiguous
`Vec` beats a `HashMap` on cache behavior — `SidebarRow` does not need `Hash`.

**The projection cache is cross-frame, and it makes filtering cheaper than
today.** `app.rs:2375` currently rebuilds the filtered rows and re-runs the
nucleo matcher *every frame* while a filter is active. The reconciler already
knows when nothing has changed, so the rows it built stay valid until the next
rebuild: paint reads them instead of recomputing. Unchanged filtering frames
therefore drop from "one fuzzy match over every row" to "one linear input
compare".

Net, against today: unfiltered frames pay one new linear compare; filtered
frames save a fuzzy match and come out ahead. There is no configuration in which
either applies, which is the point of the section below.

### Enforcing the cost budget

An argument in a design document does not survive the next refactor. Two
properties are worth enforcing, and they need different instruments:

**Zero heap allocation on an unchanged frame — a test.** Deterministic
regardless of machine or load, and it catches the exact mistake that is easy to
make: an `active_toggles()`, a `to_string()`, a temporary `Vec` slipping into
the compare. A counting global allocator, gated to the measuring thread, takes
the count from 0 to non-zero and fails.

The gate is thread-local rather than process-wide out of necessity, not
fastidiousness. `alacritree` is a binary-only crate with no `lib.rs`, so
`tests/` and `benches/` targets cannot link against it; the allocator shim has
to live in a `#[cfg(test)]` module sharing one harness process with every other
unit test, alongside a harness that allocates and app threads that allocate
whenever they like. Gating on the measuring thread is what makes a count
attributable.

**Linear rather than quadratic work — a counter, not a timer.** A wall-clock
threshold on a shared CI runner is either flaky or loose enough to detect
nothing, and the cost here is single-digit microseconds. `matches` instead
counts the records it examined, and the test asserts that a 10× larger tree does
well under 100× the work. Quadratic behavior trips that long before a timer
would notice.

**Absolute numbers — measured, not asserted.** Worth having for the PR and as a
baseline, at an ordinary size (10 projects × 5 worktrees × 3 sessions) and a
deliberately uncomfortable one (50 × 10 × 5). The estimate to beat is single-
digit microseconds per unchanged frame against an 8.3 ms budget at 120 fps; if
the measurement lands near 100 µs the estimate was wrong and the design needs
revisiting rather than shipping. A real statistical harness (`divan`) needs a
`benches/` target, hence a library target, hence moving every `mod` declaration
out of `main.rs` — a refactor that would conflict with every in-flight branch in
this repo. So this ships as an on-demand ignored test: honest numbers, no
machinery.

**What is not covered.** The gate reaches `ObservedInputs::matches` and nothing
else. `build_sidebar_snapshot` and the paint path live in `app.rs`, which cannot
be exercised without an `eframe::CreationContext`. Those stay covered by the
design (monotonic pointer, no set membership in a per-node loop) and by review.
Saying so is the point — a gate whose reach is overstated is worse than none.

**PGO is a separate question, and this harness is the wrong input to it.**
Training on a microbenchmark would tell LLVM the reconciler is effectively the
whole program, skewing inlining and code layout for a binary that actually
spends its time in egui painting and PTY reads. If alacritree ever gets PGO, the
training workload is a scripted end-to-end session — startup, sustained terminal
output, typing, scrolling, resizing, tab and workspace switching, sidebar
filtering and deletion, shutdown — where the reconciler earns its true weight.
Recorded so the two do not get wired together later on the strength of both
being called benchmarks.

### The pure function

```rust
pub fn repair(
    prev: &TreeSnapshot,
    next: &TreeSnapshot,
    cursor: Option<&SidebarRow>,
    anchor: Option<&SidebarRow>,
) -> Repair;

pub struct Repair {
    pub cursor: Option<SidebarRow>,
    pub anchor: Option<SidebarRow>,
    /// Only ever `Some` for a model-change repair, and only under `"follow"`.
    pub follow: Option<FollowTarget>,
}
```

Cursor, anchor, and terminal resolve in one call, so the "one transaction"
property is structural rather than a discipline to maintain.

The row being repaired is the **logical cursor**: the anchor when one is set,
otherwise the visible cursor. A climb leaves the user's real position in the
anchor, so removal must be judged against that, not against the ancestor the
climb parked on.

Order of resolution:

0. **Filtering ended** — nothing is filtering any more: anchor = `None`, then
   continue. Ends the episode described under *Latent filter anchor*.
1. **Anchor left the model** — the anchored row was deleted while hidden. Drop
   the anchor and repair the visible cursor by the rules below. No `follow`: the
   row was not on screen, so the user was not watching it, and the terminal's own
   `close_fallback` already covers the case where it was the active session.
2. **Anchor restorable** — the anchor is in `next.projected`: cursor = anchor,
   anchor = `None`.
3. **Cursor left the model** — sibling slide, bounded to the removed row's
   parent (below).
4. **Cursor left the projection only** — climb to the nearest ancestor present
   in `next.projected`; set the anchor to the cursor *only if the anchor is
   empty*, so successive narrowing keeps the deepest original rather than
   overwriting it with each intermediate ancestor.
5. **Otherwise** — unchanged.

### The slide is next-aware

Picking one candidate out of `prev.projected` and climbing if it fails is wrong
whenever more than one row disappears at once, which is routine — removing a
worktree takes all its sessions with it. Given `S1, S2✱, S3` and a next tree
holding only `S1`, choosing `S3` and then rejecting it lands on the parent
instead of on the perfectly good sibling `S1`.

The slide therefore resolves against the *next* tree:

1. Take the removed row's parent key and its child ordinal in the previous model.
2. Among that parent's **surviving** children in `next`, take the one at that
   ordinal — the row that slid up into the vacated slot.
3. Failing that (the removed row was last), take the nearest preceding surviving
   sibling.
4. Failing that (no siblings left), take the parent.

Then, if the landing row is absent from `next.projected` — removing a session can
make its worktree fail the `s` or `a` toggle — fall through to the climb. This is
the "otherwise the same hierarchy principle applies" case.

Resolving by parent key and ordinal rather than by previous row order also
survives a reorder: `poll_project_refreshes` can install a worktree list in a
different order, and the row that *occupies* the vacated slot is the correct
landing, not whichever row used to follow.

The parent relation reuses `left_target` (`sidebar_nav.rs:73`), so the tree shape
stays defined in one place. `Home` and `Project` both have parent `None`, making
them root siblings, so removing the only project lands on Home through the
preceding-sibling scan with no special case.

`FollowTarget` resolution: a `Session` landing follows that session; a
`Worktree`/`Home` landing with at least one live session follows that
workspace's active session, or its first remaining session when the active entry
is stale; a `Worktree` with no sessions or a `Project` header yields `None` and
today's `close_fallback` (`app.rs:4514`) verdict runs instead. Spawning a shell
the user did not ask for stays out of scope.

**`None` means "run the verdict", not "improvise".** The deferred verdict is
carried, not re-derived: `close_fallback` returns `Stay`, `Activate(main)`, or
`Home` (`app.rs:4500-4529`), and only `Activate` knows to hop to the project's
main checkout. Substituting a generic `ensure_active_session` for it would strand
`last_session_close = "navigate"` in the workspace that just emptied.

### Anchor retirement

Rather than eight sites remembering to clear it, the reconciler records what it
last wrote and retires the anchor when something else has written since. The
first draft tracked `(cursor, current_workspace)` and claimed `active_session`
must *not* be tracked. That was wrong in both directions.

**It missed same-workspace session switches.** `cycle_sessions`
(`app.rs:1143-1167`) writes `active_session` and then calls `activate_worktree`
with the workspace it is already in; when both sessions live in the same
workspace, neither tracked value changes.

The rule is stated in terms of **actions**, never key chords — every binding here
is user-configurable, so a chord names one user's setup and nothing else. The
actions that change which session is on screen are:

| Action | Handler | Default binding |
| --- | --- | --- |
| `SelectNextTab` / `SelectPreviousTab` | `cycle_tabs` (`app.rs:1113`) | Ctrl+Tab / Ctrl+Shift+Tab |
| `SelectNextSession` / `SelectPreviousSession` | `cycle_sessions` (`app.rs:1143`) | unbound |
| `SelectNextWorkspace` / `SelectPreviousWorkspace` | `activate_*` (`app.rs:1914`) | Alt+Right / Alt+Left |
| `SelectTab(n)` / `SelectLastTab` | `select_tab` (`app.rs:2215`) | Alt+1…9 |

All of them stay session-changing however they are reached — a rebound key, the
command palette (`command_palette.rs:77-80`), or `run_action` over MCP/IPC
(`mcp.rs:273`).

Observing state rather than enumerating actions is what makes that list a
description instead of a maintenance burden. "Changes which session is on
screen" *is* "changes `active_session` or `current_workspace`", so the triple
catches every entry above, every route to it, and any action added later,
without one of them being named in the reconciler.

So the tracked value is the triple `(sidebar_cursor, current_workspace,
active_session[current_workspace])`, and the two writers that motivated excluding
`active_session` mark their own writes instead:

| Site | Why it marks |
| --- | --- |
| `focus_sidebar` (`app.rs:1260`) | rewrites the cursor from terminal state; the anchor outlives a trip through the terminal by design |
| `ensure_active_session` (`app.rs:919`) | self-heals a missing active entry; no user intent |
| `adopt_active_session` (`app.rs:936`) | same, and runs from paint (`app.rs:6413`) |

Three marked sites, against roughly nine that would otherwise each need a
retirement call — sidebar clicks (`app.rs:2951-2976`), `cycle_sessions`,
`cycle_workspaces`, the palette, notification activation, IPC, and the three
search exits. Those nine stay unedited.

**Same-value writes remain unobservable, and that is why the episode rule
exists.** A confirm that lands on the row the climb already chose writes an
identical cursor; no comparison can see it. Retiring the anchor when filtering
ends covers every such case without asking the sentinel to recover intent from
state it cannot distinguish.

### Invocation

**Twice per `update`, not once.** One reconciler, two call sites, because
`update` has two points where the model can change and only one of them is
before paint:

1. After `process_session_events` (`app.rs:6365`) and before painting. This
   catches everything driven by the keyboard, IPC, `poll_pending_deletes`
   (`app.rs:6343`), and the PTY event drain — every deliberate delete. Zero
   added latency: the repair lands in the same frame the removal does.
2. At the very end of `update`, after `reap_exited_sessions` (`app.rs:6477`).
   That function removes nothing itself — it calls `close_session` per exited
   PTY, so under `"follow"` it produces a deferred verdict with no pass left to
   apply it. Paint-time clicks on a session row's `×` land here too.

The second call is not a second implementation; it is the same method, and when
nothing changed it returns after one `ObservedInputs` compare. Paying a
microsecond to remove a frame of latency is the right trade.

What the second call cannot do is repaint a frame that already painted. A shell
that exits after the central panel has drawn shows its replacement on the next
frame no matter who observes it — that is immediate-mode rendering, not this
design. The deferral therefore always requests a repaint, so that next frame is
issued immediately rather than waiting for unrelated input. Without it, a
deferred verdict is not a lag but a hang: the terminal would sit on the "no
session" placeholder until the user touched something.

### The three unavoidable deferrals

Observation cannot un-spawn a PTY, un-navigate, or un-reset a cursor. Three
paths act before any observer could see the state that motivated them, so each
hands its decision to the reconciler.

**`after_filter_changed` (`app.rs:1434-1441`) — the one the first plan missed.**
It calls `ensure_cursor` the instant a filter changes, so the cursor is already
sitting on row 0 by the time the reconciler looks. The reconciler would then see
a cursor it did not write, retire the anchor, and have no way to learn which row
had been hidden. **Without this the filter behavior — the primary ask — does not
work at all.**

It is the one of the three that is not conditional. With no `"reset"` mode there
is no configuration in which it still repairs eagerly, so it is *deleted* rather
than guarded — its single caller, the `Outcome::FilterChanged` arm of
`apply_filter_outcome` (`app.rs:1425`), becomes empty. `sidebar_nav::ensure_cursor`
keeps its other callers and stays.

The consequence is worth stating rather than discovering: nothing catches a
filtered-out cursor any more except the reconciler. If the reconciler fails to
run, the cursor stays wherever the filter left it, with no fallback beneath it.

**`close_session` (`app.rs:964-995`)** runs `close_fallback` immediately, before
an observer could return a `follow`. It stores the verdict instead of acting on
it.

**`run_pending_delete` (`app.rs:5576-5585`)** drops the worktree's sessions and
calls `activate_home`, which can spawn a home shell, before any observer sees
anything. Under `"follow"` that spawns a shell the user immediately navigates
away from.

The worktree delete needs one thing beyond deferral. The git operation is
asynchronous, so the worktree stays in `projects` after its sessions are gone:
the cursor still points at a row that is still in the model *and* still
projected, `repair` reports no change, and `follow` is `None`. Treating "no
active session" as evidence that a deferred verdict is due would then spawn a
fresh PTY **inside the directory being deleted** — recreating the process whose
removal unblocked the delete. So `run_pending_delete` marks the worktree as
logically removed when it defers, and the reconciler treats it as absent from the
model from that moment. The deferred verdict is applied because it was recorded,
never because the app happens to have no active session.

`close_session` and `run_pending_delete` stay gated on `follows()`; only the
filter path is unconditional.

**No mode exercises the old immediate paths any more.** That is the second cost
of dropping `"reset"`, and it lands squarely here: the suite cannot compare
deferred behavior against eager behavior, because eager behavior no longer
exists to run. Deferral is supposed to change *when* these act and nothing else,
and the only thing that can confirm it is a named manual pass — no frame showing
the "no session" placeholder, no repaint waiting on a keypress, and
`last_session_close = "navigate"` still landing on the project's main checkout.

Total footprint: two reconciler call sites, one deletion, two deferrals, three
marked writes.

## Testing

TDD throughout: each behavior gets a failing test first, confirmed failing for
the right reason, before the implementation lands.

**`repair` over before/after snapshot pairs** — the bulk of the coverage, and
writable without an `eframe::CreationContext`:

- the three cases from the reference tree
- a worktree's only session → the worktree row; a project's only worktree → the
  project row; the only project → Home; the first project with siblings → the
  next project, not Home
- removing a worktree also removes its session rows, so the slot holds the next
  worktree rather than an orphaned session
- collapsing a project is a projection change, not a removal — the cursor climbs
  and anchors rather than sliding
- a session dropping below the listing threshold is likewise a projection change
- filter narrows past the cursor, then widens → the anchor restores
- successive narrowing keeps the deepest anchor, not the intermediate ancestor
- the landing row is itself filtered out → falls through to the climb
- `follow` targets for each landing shape: a session landing, a workspace landing
  with a live active session, a stale active entry falling back to the first
  remaining session, and an empty worktree yielding `None`
- **two siblings removed at once** → the surviving sibling, not the parent
- **a worktree reorder plus a removal** → the row now occupying the vacated
  ordinal, not the row that used to follow
- **the anchored row is deleted while hidden** → the anchor is dropped, the
  visible cursor is repaired, and `follow` stays `None`
- **filtering ends** → the anchor retires even when the cursor value is unchanged

**Snapshot change detection** — unchanged inputs short-circuit; each tracked
input in isolation triggers a rebuild. "Each" is literal: project order, root,
name, expansion and worktree list; session order, workspace, id and attention;
the listing mode; the query; each toggle bit. A test that varies three of those
and calls it done is how the `active_toggles` allocation survived review.

**Anchor retirement** — the triple is compared, not the pair: switching between
two sessions in the *same* workspace retires the anchor, while
`ensure_active_session`/`adopt_active_session`/`focus_sidebar` marking their own
writes do not.

**Discovery** — covered by the prerequisite branch's own plan, not here.

**Steady-state cost** — an unchanged frame allocates nothing, filtered or not,
at an ordinary and an uncomfortable tree size; a 10× larger tree does well under
100× the record comparisons. Prove RED by putting an allocation back in
`matches` — `active_toggles()`, or simply a `to_string()` — and confirming the
count goes from 0 to non-zero. An allocation gate that has never failed is not a
gate.

**`config.rs`** — default is `"preserve"`, both values parse, an unknown value
falls back, and the retired `"reset"` parses to the default rather than
refusing: a config file carrying it must still start.

**Manual GUI check** — delete and search flows under both settings, the deferral
parity pass described above, and the timing harness numbers recorded, in the
isolated verification lab before the PR.

The existing `app.rs` tests are helper-level (`close_fallback`, `plan_move`) and
there is no app fixture: `AlacritreeApp::new` (`app.rs:487`) requires an
`eframe::CreationContext`, starts IPC, loads persisted state, and spawns a
PTY-backed Home session. Pushing the decision into `repair` is what makes these
assertions full state transitions rather than leaf-function checks.

That constraint is a reason to move logic into pure functions, not a licence to
test predicates that restate their own implementation. A test asserting that
`defers(mode) == mode.follows()` proves nothing about either deferral; the
deferral's *effect* — verdict recorded, cursor left alone, repaint requested —
belongs in the pure transition. The filter deferral gets no predicate test at
all, because it is unconditional and the only thing such a test could assert is
that a constant is constant.

Anything a pure function cannot reach is a named manual GUI step rather than a
test that pretends to cover it. That set grew when `"reset"` went away: it now
includes the deferral parity check, which used to be covered by simply running
the default mode.

## Out of scope

- Terminal-follow on filter changes, on anchor restore, or on arrow-key
  navigation.
- Spawning a session for an empty worktree the cursor lands on.
- The right-hand git sidebar's cursor (`git_nav`), which has its own model and
  repairs per focused render (`app.rs:3147`).
- Persisting the anchor across restarts.

## Decided

Recorded because each closed a question that shaped the design, and a later
reader will otherwise reopen them.

- **`"preserve"` is the default and `"reset"` is gone.** See *One config key,
  and the parity exception it takes* for what that buys, what it costs, and the
  rejected alternative.
- **The WSL discovery fix ships as its own branch and PR, off `master`, first.**
- **The anchor retires on any observed `active_session`/`current_workspace`
  change**, including a notification click and an MCP- or IPC-driven switch.
  Something did move the terminal off the row the anchor was taken for; treating
  the route as significant would mean marking those writes, which is exactly the
  per-site bookkeeping the observer exists to avoid.
- **The cost budget gets a real gate**, not a prose claim. See *Enforcing the
  cost budget*.

## Unresolved questions

1. **Does the `a` toggle need special handling?** While `a` is active, a
   background session flipping attention changes the projection with no user
   input, so the climb fires attributing a landing to no action at all. Under
   the reconciler this is observed promptly rather than on the next keystroke,
   which makes it more visible than before. The anchor makes it recoverable.
   Deferred: implement without special handling and revisit if it bites.
2. **Should a non-authoritative discovery surface anything to the user?** A WSL
   project can now show a stale worktree list, silently, for as long as the
   distro is down — strictly better than collapsing it, but still a lie with no
   tell. Carried in the prerequisite branch's plan; out of scope here.
3. **When the cursor and the terminal disagree, which one is "where I am"?**
   `"preserve"` deliberately lets them drift: the cursor slides to a sibling
   while the terminal stays put. `"follow"` collapses the two. Nothing in this
   design decides which is the better default beyond "don't change what the
   terminal shows unless asked", and daily use under `"preserve"` is the only
   thing that will answer it.
