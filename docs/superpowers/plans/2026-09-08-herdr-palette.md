# Herdr palette implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task by task. Steps use checkbox syntax for tracking.

**Goal:** Give native and herdr sessions compact, identifiable palette rows, restore separate unattached-session grouping and full hover details, and correct native Windows agent selection.

**Architecture:** Keep the existing palette grid and action dispatch. A pure presentation layer supplies session titles, subtitles, middle cells, explicit search text and tooltips; the egui painter lays out those fields. The Windows probe fix is independent of presentation.

**Tech Stack:** Rust, egui/eframe, existing sysinfo process snapshot, schemars configuration.

**Spec:** C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/fable_01.local.md. User approved implementation after reviewing two refinements: omit the optional foreground-process feature; bound the session middle cell as well as its title.

## Global Constraints

- Two session sections on the existing description | action | keys grid: "Open sessions" and "Herdr sessions".
- Open sessions holds every alacritree session, attached herdr sessions included. Herdr sessions holds every unattached pane herdr_agent_listing returns. Existing show_unmatched and show_panes settings retain their behavior.
- Every session row has a title line, a location line, and a middle cell. Title uses theme.font_normal, word wrapping to at most two rows, then ellipsis. Location uses the palette small dim style, exactly one clipped row. Session middle cells must also be bounded so a narrow palette cannot exceed three text rows plus padding.
- PaletteColumns does not change. Action rows keep their current layout, elided-only tooltip and search.
- Middle cell: lowercase agent kind and status joined with " · "; omit the kind when the title contains it case-insensitively. Shell rows say "shell". Keep the actual detected kind when a different agent name is in the title.
- Project labels already contain user-selected glyphs; paint no OS/distro glyph of our own. Attached herdr rows prefix their location with the configured ui.icons.herdr glyph. Distro names appear only in the tooltip.
- Search session painted strings plus agent kind and the word "herdr" for managed rows. Tooltip-only full paths, distro names and terminal IDs do not enter the haystack. A pane ID is searchable when it is visibly used to distinguish a generic row.
- Every session/herdr row has a whole-row tooltip regardless of elision or palette_marks. The status mark keeps painting under palette_marks but does not own a competing tooltip on these rows.
- Preserve session_row_name and herdr_display_name title precedence, while recognizing missing raw titles before their generic fallback. A generic title is missing, equal to kind case-insensitively, equal to cwd, or equal to cwd's last component.
- Native foreground-process display is omitted. Do not add name fields to Signals, AgentCache or Session. Native locations use the spawn workspace; herdr supplies its own cwd.
- No new config gate. The approved design explicitly permits changing native session rows. No vendored-crate edits, new dependency, live GUI/server mutation, installation, PR, merge or push during implementation.
- Existing app.rs and command_palette.rs uncommitted changes are the failed first iteration and must be replaced by this design, not committed as a prerequisite. Preserve all already committed herdr synchronization and configuration defaults work.
- Use absolute paths and PowerShell, no stash/reset/clean. Claim touched files via lockm. Conventional commits include Co-Authored-By: GPT-5.6 (Codex) <noreply@openai.com> for a GPT-5.6 implementer; use the actual model family if different.

## Execution environment

Worktree: C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette.
Baseline HEAD: 342b14f4ef100f382d2fcc02f38a13f36dedd265.
Shared lock identity: 01a081b5-bc98-7c91-bdee-da9628f2ed2f. Use lockm acquire/release --as ID and absolute file paths from this worktree, requiring elevated execution for registry writes.
Build config: C:/Users/Lev/Git/github/alacritree/devkit.local.toml.
Canonical checks: devrun --config CONFIG -C WORKTREE task fmt; task test --env RUSTC_WRAPPER=; task clippy --env RUSTC_WRAPPER=.
The wrapper bypass is per-command: shared sccache failed previously. Do not repair its global state.
Use a controller-provided temporary devkit config to add focused nextest tasks without changing the real config. Cargo/registry commands need require_escalated in this sandbox. Run compilation checks sequentially; concurrent nextest/clippy previously invalidated a test executable.
Report full RED/GREEN command and output in the task report and keep logs in this plan's workspace. No worker spawns subagents.
The baseline was tested earlier in this session (1,507 passed, 16 skipped) and its files are unchanged since; do not repeat unchanged baseline tests.

## Task 1: Preserve process hierarchy when detecting Windows agents

**Files:** Modify and test alacritree/src/session.rs only.
**Consumes:** process_tree_pids, windows_process_probe::scan, ScannedTrees and remembered_agent.
**Produces:** The same public session/activity interface with corrected internal candidate ordering.

- [ ] Read the actual scan and cache path. process_tree_pids returns root-inclusive breadth-first order. scan currently sorts that vector by numeric PID before building name candidates. Name matching must see breadth-first order, including the root itself when it directly launches an agent.
- [ ] Add a test that drives the same production candidate-building/selection boundary scan uses with this snapshot: root powershell.exe PID 15256, child claude.exe PID 34352, Claude child codex.exe PID 32372. The expected agent is claude. A test that merely calls process_tree_pids and agent_name_by_name separately while scan still sorts is insufficient.
- [ ] Run that test before changing selection and record a behavioral failure selecting codex. If a pure helper extraction is needed to reach the production boundary, first preserve old behavior in it and establish RED there.
- [ ] Preserve breadth-first order for executable-name and command-line matching. Build a separate sorted PID vector for BOTH remembered_agent lookup and ScannedTrees storage. Keep the agent cache comparison order stable and existing query throttling intact.
- [ ] Test direct-agent root selection and cache equality across different snapshot sibling order where needed by the changed production boundary. Run focused Windows probe tests and then the configured full suite once.
- [ ] Self-review and commit only session.rs as fix(session): preserve agent process hierarchy. Explain in the body that native claude.exe wins over a lower-PID child codex.exe and that npm/node wrappers still have the existing name-pass-before-command-line limitation.
- [ ] Write the task report, release file claims, and return status, commit and test summary.

## Task 2: Build compact session content, sections and explicit search

**Files:** Modify and test alacritree/src/command_palette.rs and the palette presentation helpers/construction in alacritree/src/app.rs. Leave paint_palette_row and PaletteColumns to Task 3.
**Consumes:** Unchanged Session::activity, session_row_name, herdr_display_name, path_style, workspace_label, Managed::herdr/managed_tooltip and existing action dispatch.
**Produces:** PaletteItem fields public subtitle: Option<String> and hover: Option<String>, defaulting to None for non-session rows; PaletteSection::HerdrSessions with exact title "Herdr sessions". All session and herdr items carry Some subtitle (including empty text when no location information remains) and Some hover. Constructors/builders may evolve narrowly; keep all call sites compiling and describe their final interfaces for Task 3.

- [ ] Restore the unattached section and replace the failed first iteration's metadata-heavy primary and herdr open/attach middle labels. Preserve the fuzzy ranker's existing group ordering behavior: natural order while unfiltered, best-matching section first under search.
- [ ] Build presentation in small pure functions with tests through actual palette item construction and CommandPalette::rank/group where search/grouping is involved. No generic framework, alternate data registry, probe expansion or unrelated sidebar rename.
- [ ] Implement this title/location table with foreground-process content omitted:
  | Case | Title | Location |
  | --- | --- | --- |
  | Specific title | resolved title | attached herdr glyph when applicable, then workspace label |
  | Generic title, matched workspace | workspace label | attached herdr glyph; unattached herdr uses pane ID; native may have an empty location |
  | Generic title, no workspace | existing path_style abbreviation of herdr cwd, or Home for native | pane ID for unattached herdr; attached herdr keeps its glyph |
  Use nonempty foreground_cwd then cwd for herdr location and generic-title recognition; handle comparison against either reported directory when they differ. When a herdr pane has neither usable title nor cwd/workspace, use Home and retain the pane ID disambiguator.
- [ ] Specific-title rows show workspace context; no-workspace herdr rows may use abbreviated cwd as location when the title is specific, so they remain identifiable. Do not duplicate a promoted title in its subtitle.
- [ ] Use Project::display_name via workspace_label so configured project glyphs/renames survive. Resolve the configured herdr glyph using the existing icon data/fallback. Do not invent Windows/WSL glyphs.
- [ ] Middle text follows Global Constraints. Tests include Claude Code + kind claude -> idle; Claude Code + kind codex -> codex · idle; a plain shell -> shell. Virtual scratchpad rows keep a truthful non-shell identity if their SessionKind already provides it; no process probing for them.
- [ ] Assemble whole-row tooltips, one fact per line, for all native and herdr rows: full resolved title, kind/status when available, side (native or wsl:distro), full workspace path in that side's spelling, herdr cwd, herdr pane and terminal IDs, and activation behavior. Use managed_tooltip output for attached/unattached herdr; native activation says "switch to this session". Since foreground feature is omitted, do not include that field. Retain details even with palette_marks disabled.
- [ ] Search is constructed explicitly from primary, subtitle, secondary, agent kind and herdr tag. Do not include hover text or unrelated metadata. Full pre-ellipsis title/location text is searchable; drawing-time ellipsis does not modify model data. Include displayed pane IDs, exclude terminal IDs and tooltip-only distro/full paths.
- [ ] Pin the two duplicate unmatched panes titled chezmoi in ~/.local/share/chezmoi: both titles become ~/.l/s/chezmoi, subtitles distinguish w7:p1 and w7:p3, middles preserve codex · working and codex · idle. Also test missing raw title, directory-as-title, generic matched workspace, configured labels/glyph, native Home, and specific titles.
- [ ] Establish RED for changed row construction/group/search, implement, run focused tests, full suite once, and commit app.rs/command_palette.rs as feat(palette): clarify session row content. This commit may include the reviewed first-iteration hunks only as transformed to match this task; no separate commit preserving the failed iteration.
- [ ] Report interfaces, test evidence and source changes; release claims.

## Task 3: Paint bounded two-line rows with persistent hover

**Files:** Modify and test alacritree/src/app.rs palette layout/paint code. Adapt only the small PaletteItem interface if a rendering requirement cannot be carried by Task 2 fields.
**Consumes:** Task 2 public subtitle and hover, existing primary/secondary/keys, and unchanged PaletteColumns.
**Produces:** Completed session presentation, with action rows' behavior intact.

- [ ] Add regression checks through production paint_palette_row/egui layout for subtitle placement, row hit target/height and tooltip eligibility. Existing headless egui tests may be reused. Use deterministic fonts/widths available to the test environment. Do not substitute tests of a mirrored layout formula for the actual paint/layout path.
- [ ] For session rows, word-wrap primary to at most two rows, then ellipsize. Preserve the existing long-token fallback but bound both word and anywhere attempts. Lay out subtitle as exactly one small dim line with ColumnWrap::Clip; never route it through prose_galley.
- [ ] Bound session middle cells to at most three text rows and ellipsize overflow. Session keys remain empty. Compute row height from the taller of title plus subtitle, action and keys galleys plus existing vertical padding; maintain existing top alignment and grid widths. Reserve the subtitle line for session items even when its text is empty. Preserve non-session action layout.
- [ ] Paint configured herdr glyph already in subtitle as part of its content, preserving user project labels. If configured icon font resolution needs its own text section, implement it narrowly and report the interface adjustment.
- [ ] Attach item.hover to the whole row whenever Some, independent of elision and palette_marks. Remove the competing status-mark hover target for those rows; retain existing behavior for action rows and any item without explicit hover.
- [ ] Test comfortable and narrow widths, a long path/branch token, title overflow after two rows, a long middle cell and no-elision tooltip availability. Confirm action rows still get elided-only hover. Cover keyboard ranking/selection separately through unchanged model tests as appropriate.
- [ ] Run focused tests through the configured task runner, then final configured fmt/test/clippy sequentially. Inspect a headless render artifact if feasible without launching the daily-driver app; record honestly any live GUI limitation.
- [ ] Self-review and commit as feat(palette): render bounded session details. Write report with exact test/log evidence, release claims and return.

## Task 4: Reconcile attached rows against confirmed remote terminal inventory

**Files:** Modify and test alacritree/src/herdr.rs, alacritree/src/session.rs, and alacritree/src/app.rs lifecycle integration only. No broad module extraction. This task follows the user's later approved runtime fix and is independent of the proposed pane transport.
**Problem:** Closing the remote tab/process leaves the full herdr UI attach client alive. Local ChildExit therefore cannot establish the row's lifetime. An agent disappearing from agent list can mean it returned to a live shell, so that list cannot establish terminal deletion either.
**Consumes:** Stable herdr::HerdrKey { side, terminal_id }, Session.herdr_key, endpoint polling, existing app close_session cleanup/navigation.
**Produces:** Successful full terminal inventories and automatic removal of attached rows whose terminal is confirmed absent in an inventory started after attachment. Display filters and existing attachment modes/default remain unchanged.

- [ ] Write a behavioral regression through the real listing JSON parser, endpoint adoption and live Session binding path. Use pending_shell without opening its PTY for test sessions. Feed a valid pane list containing a shell terminal and an agent terminal, bind local sessions, then feed a later valid pane list missing one terminal. The production reconciliation path must choose only the absent session for removal. Capture RED for the stale-row bug before adding reconciliation, rather than a missing-symbol/setup failure. Small test-enabling helpers are permitted when they are the actual production path; do not mirror the predicate in test-only code.
- [ ] Implement strict full-inventory parsing separately from rendering metadata. A valid result.panes array supplies terminal identity even when optional title/status/cwd data is missing. A malformed envelope, error envelope, absent/wrong-type panes member, malformed terminal identity, failed process, failed job or unavailable endpoint supplies no deletion evidence. An explicit empty panes array is valid evidence. Agent-only output is never a complete pane inventory.

```rust
// Public interface shared by the cache and app reconciliation.
pub struct PaneInventory {
    pub sampled_at: std::time::Instant,
    pub terminal_ids: std::collections::HashSet<String>,
}
// Bind the sampled_at value when starting the actual pane-list request.
// Parsing produces membership, independent of optional display metadata.
fn parse_pane_inventory(stdout: &str) -> Result<std::collections::HashSet<String>, PollError>;
```

- [ ] Add inventory polling for sides that currently own attached local sessions, respecting integrations.herdr.enabled and the configured poll interval. Keep the visible listing controlled by show_panes. The simplest independent inventory job per relevant endpoint is acceptable; reuse a successful full pane listing when doing so avoids another process without weakening provenance. Keep timestamps tied to the request that produced the answer, including in-flight listing changes. Invalidate deletion evidence after any failed or malformed poll. Do not infer deletion when a WSL endpoint disappears. No polling, jobs, or allocations for attached-session inventory when there are no herdr attachments.

```rust
impl EndpointCache {
    pub fn inventory(&self) -> Option<&PaneInventory>;
}
// A confirmed inventory may only remove a binding it could have observed.
// inventory.sampled_at > binding_started_at && !inventory.terminal_ids.contains(id)
```

- [ ] Record the binding time when a Session receives its herdr key, at the actual successful local attach/open path. Add a narrow Session binding method and use it at every production key-assignment site. Treat missing provenance conservatively. A listing requested before that binding cannot close it. Reattaching the same remote terminal gets a fresh binding time.
- [ ] Run reconciliation after endpoint adoption and before focus synchronization. Remove confirmed absent local sessions through the existing close_session path so active-session maps, sidebar cursor repair, and last-session policy stay consistent. Ignore stale focus completions for removed local session IDs; clear pending local bookkeeping that could resurrect a removed row. Closing a local row only releases its local client, with no remote pane/tab/process kill. Preserve live-shell rows when their agent exits. Inventory polling failures, server refusal/shutdown, unsupported pane listing, and endpoint absence cannot by themselves remove rows. Preserve the existing local client-exit/detach policy in this task; distinguishing a connection failure from an intentional detach belongs to the separately proposed transport.
- [ ] Add diagnostic logs for attachment binding, accepted inventory membership changes, and confirmed row removal, naming local session ID, side, terminal ID and evidence time. Log transitions, not every frame. Terminal contents and keystrokes do not belong in logs.
- [ ] Cover through the production parser/adoption/reconciliation path: valid empty inventory removes bound rows; agent exits into shell but terminal ID remains; malformed/error reply preserves rows; successful inventory followed by failure cannot close from stale evidence; pre-attachment inventory is ignored; identical terminal strings on different sides cannot close each other; missing display metadata does not erase identity; show_panes=false keeps shell rows hidden in the unattached list while using full inventory for attached rows; repeated unchanged frames do not keep requesting immediate polls. Existing close/sidebar tests cover navigation mechanics, and add only the lifecycle wiring assertion needed to prove this caller reaches that cleanup.

```text
Inventory input: {"result":{"panes":[{"terminal_id":"term-shell","pane_id":"w1:p1"}]}}
Expected: term-shell survives even with no agent fields; an older bound term-gone is removed.
Inventory input: {"result":{"agents":[]}}
Expected: no deletion evidence.
Inventory input: {"result":{"panes":[]}}
Expected: a later successful sample confirms all previously bound IDs absent.
```

- [ ] Use the configured runner for focused RED/GREEN, then fmt and the full test suite once, sequentially. Retain commands, exit codes, test summaries and output logs. The existing main devkit config has canonical tasks; an ignored copied config may add a nextest expression for these tests. Escalate read/build access if the sandbox makes a valid worktree appear absent. Shared compiler-wrapper failures get a per-command bypass, never a global tool repair.
- [ ] Self-review, selectively commit as fix(herdr): remove closed remote session rows, append exact evidence to task-4-report.md, release file claims and return the short SDD report. Do not add the proposed pane transport in this task. Its input-mode contract is under independent investigation.

## Completion

After each task the controller packages the full BASE..HEAD diff for a fresh spec/quality reviewer and resolves findings through the SDD loop. A final whole-implementation review uses baseline 342b14f4ef100f382d2fcc02f38a13f36dedd265, covering the palette tasks and approved lifecycle correction. The controller finishes with committed work on this branch; merging/pushing/installing are separate actions unless the user explicitly steers this implementation run to them.

## Unresolved questions

None. Implementation rulings are recorded in the SDD ledger.
