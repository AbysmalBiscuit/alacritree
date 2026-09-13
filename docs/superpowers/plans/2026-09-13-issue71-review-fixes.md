# Issue 71 review fixes

Worktree: `C:\Users\Lev\Git\github\alacritree-worktrees\refactor-app-split-the-god-module-into-field`
Branch: `refactor-app-split-the-god-module-into-field`, based on `44aa09b1`, head `e807c7e0` when this plan was written.
Spec: https://github.com/AbysmalBiscuit/alacritree/issues/71 (split `app.rs` into field-owning submodules; `dispatch_action` split "so each arm's body sits with the state it touches"; one submodule per commit).

## Global Constraints

- Behaviour is unchanged. Every `NamedAction` does exactly what it did at `e807c7e0`.
- Moved comments stay verbatim. New or rewritten comments use one space after a full stop and separate clauses with periods or commas: no em dashes, en dashes, or hyphens used as dashes.
- Build, test, lint and format only through devkit: `devkit run task check|test|fmt|clippy --dir <worktree>`. Tests all pass, fmt leaves the tree clean, clippy reports 51 warnings.
- Claim a file before editing it: `DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm acquire <abs path> --note "<why>"`.
- Absolute paths, `git -C <worktree>`, `rg` rather than grep, never `git stash`. No push, no PR, no `all-features`.
- Leave the untracked `foo.txt` in the worktree alone.
- Each fix lands as a `git commit --fixup=<sha>` against the commit that introduced the code, so Task 3 can autosquash. Commits carry `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>` (or the dispatching model's own name).

## Task 1: Make action dispatch panic-free and owned by the submodules

Files: `alacritree/src/app/actions.rs`, `app/focus.rs`, `app/git_panel.rs`, `app/herdr_glue.rs`, `app/sidebar.rs`.

Current state: `dispatch_named_action` in `actions.rs` lists variants in `action @ (A | B | ...)` groups and forwards each group to `dispatch_general_action` (actions.rs), `dispatch_herdr_action` (herdr_glue.rs), `dispatch_sidebar_action` (sidebar.rs), `dispatch_git_action` (git_panel.rs) and `dispatch_search_action` (focus.rs). Each of those re-matches and ends in `_ => unreachable!()`, so a variant listed in the outer group but missing from the inner match panics at runtime. `dispatch_general_action` is a 133-line bucket of arms with no single owner.

Change:
1. Every owner dispatcher has the shape of the existing `dispatch_git_filter`: `pub(super) fn dispatch_<owner>_action(&mut self, ..., action: NamedAction) -> bool`, one arm per variant it owns returning `true` after doing the work, and `_ => false`. No `unreachable!` anywhere in dispatch.
2. `dispatch_named_action` keeps the one-expression arms that need no owner (paste, tab and session cycling, `NoOp`, `ReceiveChar`, the session-row toggles, workspace cycling, scratchpad, add project, refresh, palette, `FocusTerminal`, `FocusLeft`, `FocusRight`, `Quit`, `Minimize`). Everything else goes through a chain like `if self.dispatch_sidebar_action(ctx, action) || self.dispatch_git_action(action) || ... { return; }` followed by `dispatch_filter_or_other`, replacing the `action @ (...)` groups so the variant list lives only in the owner.
3. Delete `dispatch_general_action`. Move its arms to owners: `ToggleLeftSidebar`, `ToggleSidebarFocus`, `CloseSession` (reads the sidebar cursor), `FocusProjectsSidebar` to `sidebar.rs`; `ToggleRightSidebar`, `FocusGitSidebar`, `RefreshPrStatus` to `git_panel.rs`; `SpawnProfile`, `CloseExitedSession`, `SpawnNewInstance`, `Copy`, `CopySelection`, `ClearHistory`, `ToggleFullscreen`, `ToggleMaximized` stay in `actions.rs` as a `dispatch_session_action -> bool` if no submodule owns their state. Arm bodies and their comments move verbatim.
4. Inline `dispatch_unsupported_action` back into `dispatch_action`'s `BindingAction::Unsupported(name) => log::debug!(...)` arm.

Done when: `rg -n 'unreachable!|dispatch_general_action|dispatch_unsupported_action' alacritree/src/app` is empty; `rg -n 'action @' alacritree/src/app/actions.rs` is empty; `dispatch_named_action` and every owner dispatcher is under 100 lines; the existing `dispatch_action` tests in `app.rs` pass; all devkit tasks pass. Fixups target `5bf6bda1` (owner split) and `e1b6bbb8` (introduced `dispatch_unsupported_action`).

## Task 2: Two comment fixes

1. `alacritree/src/app/model.rs:1182`: the `HerdrRowData` doc says rendering doesn't borrow `self.endpoints`. The field lives at `self.herdr.endpoints`. Fixup target `53edab69` (herdr extraction, where the rename was folded).
2. `alacritree/src/app/focus.rs`: the comments rewritten at `98054397` put two spaces after full stops. Exactly these: the `written` field doc ("What the reconciler itself last wrote." through "overtaken."), the `reconcile_sidebar_focus` doc ("Called twice per `update`." through "budget: there is no setting that skips this."), and the `sidebar_search_confirm` doc ("Selecting a row never activates it." through "search mode."). Use one space after each full stop in those three docs only, rewrapping to the 100-column comment width. Fixup target `98054397`.

Done when: `rg -n 'self\.endpoints' alacritree/src/app/model.rs` is empty, those three focus.rs docs contain no `.  ` sequence, fmt is clean.

## Task 3: Fold fixups and add trailers

1. `git -C <worktree> rebase --autosquash 44aa09b1` with `GIT_SEQUENCE_EDITOR=true` so no editor opens. Resolve nothing by guessing: if a conflict appears, abort and report it.
2. Add `Co-Authored-By: GPT-5 (Codex) <noreply@openai.com>` to every commit in `44aa09b1..HEAD` that has no `Co-Authored-By` trailer, using `git rebase 44aa09b1 --exec 'git commit --amend --no-edit --trailer ...'` guarded so commits that already carry a trailer are left as they are.
3. Run all devkit tasks on the final HEAD.

Done when: `git log --format='%h %s | %(trailers:key=Co-Authored-By,valueonly)' 44aa09b1..HEAD` shows no `fixup!` subject and a trailer on every commit; `git diff <pre-rebase head> HEAD` is empty; tests, fmt and clippy pass.
