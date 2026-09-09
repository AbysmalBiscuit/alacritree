# herdr Module Split Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Break `alacritree/src/herdr.rs` and the herdr-shaped code stranded in `app.rs` into a `src/herdr/` directory of small, single-responsibility files, changing no behavior.

**Architecture:** Every task is a pure move. Code is cut from one file and pasted into another with no edits except `use` lines, visibility keywords, and module declarations. `mod.rs` starts holding everything and ends holding only submodule declarations and re-exports, so call sites keep reading `herdr::Agent` and never change. Correctness is established by the existing suite plus a mechanical signature diff, not by new tests.

**Tech Stack:** Rust 2024, edition 2024, MSRV 1.85. No new dependencies.

## Global Constraints

- **This is a shared checkout.** Claim every file before editing it: `DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm acquire <abs path> --note "<why>"`. `lockm` is project-root scoped and takes no `--dir`.
- **Pure move rule.** A task's diff may contain only: moved code, `use` statements, visibility keywords, `mod` declarations, and `pub use` re-exports. No renames, no signature changes, no logic edits, no reordering within a moved block. If the code looks wrong while you are moving it, leave it wrong and say so in your report.
- **Comments move verbatim.** Do not reword, add, or delete a doc comment while moving it. Comments must never narrate the move: no "moved from", no "this PR", no "previously".
- **Line numbers anchor declarations, not attribute blocks.** Every line number in this plan names the line an item is declared on. Move the item together with everything attached above it: its doc comment and its `#[derive(...)]` or other attributes. For example `struct Envelope` is listed at 153, and its `#[derive(Deserialize)]` on 152 moves with it.
- **Commit format.** Conventional Commits, imperative subject under 72 characters, lowercase after the colon, no trailing period. Every commit ends with the trailer `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.
- **One commit per task.** Stage selectively with explicit paths; never `git add -A`.
- **Tooling.** `rg` for content, `fd` for filenames. `grep` and `find` are hook-banned. `rg` has no recursive flag; `-r` means `--replace`. Write absolute paths into commands rather than `cd`-ing first; use `git -C <abs repo>` for git.
- **Formatting.** `RUSTC_WRAPPER= devrun task fmt` runs nightly rustfmt (the long form is `devkit run task fmt`). Stable `cargo fmt` silently ignores ten of the fifteen options in `rustfmt.toml` and reformats untouched files, so never use it.
- **Working directory.** `C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette`, branch `feat/herdr-palette`.
- **No PR.** Do not push or open a pull request. The user asks for that separately.

---

## Starting state

The working tree already has an unfinished, non-compiling reorganization that Task 1 must resolve:

- `alacritree/src/herdr.rs` is deleted (staged as `D`).
- `alacritree/src/herdr/herdr.rs` is the full 2099-line original, untracked.
- `alacritree/src/herdr/app.rs` and `alacritree/src/herdr/herdr_e2e.rs` are empty placeholder files, untracked.
- `alacritree/src/herdr/mod.rs` contains `pub use self::herdr::*` with no semicolon and no `mod herdr;`, so it does not compile.
- `test.sock` and `test-client.sock` at the repo root are stray artifacts from a herdr probe, untracked, and must be deleted.

`alacritree/src/main.rs:28` already declares `mod herdr;`, so `src/herdr/mod.rs` is picked up with no change to `main.rs`.

## File structure

| File | Responsibility |
|---|---|
| `src/herdr/mod.rs` | Submodule declarations and explicit re-exports. No code of its own at the end. |
| `src/herdr/model.rs` | The vocabulary: `Side`, `Agent`, `Status`, `Indicators`, `Settings`, `HerdrKey`, `Listing`, `PollError`, and the pure functions over them. No serde. |
| `src/herdr/wire.rs` | What herdr sends: the serde types and the parsing that turns them into model types. |
| `src/herdr/cli.rs` | Process invocation: argv builders, the spawn helpers, and the commands that map one-to-one onto herdr subcommands. |
| `src/herdr/settings.rs` | Reading and interpreting herdr's own `config.toml`, including the spawn that fetches it, because that whole path exists only to produce a `Settings`. |
| `src/herdr/poll.rs` | Endpoint reachability, caching, and poll scheduling. |
| `src/herdr/view.rs` | The focus-reconciliation model moved out of `app.rs`. Named for the `HerdrView*` types it holds and herdr's own "shared view" concept. |

`src/herdr/e2e.rs` is deliberately not created here. It belongs to the follow-focus work that comes after this plan.

## Verification method

A pure move is verified mechanically, not by new tests. Two checks run at the end of every task.

**Check A, the signature diff.** The set of top-level item signatures across the herdr module must be identical before and after, ignoring which file they live in.

```sh
RUSTC_WRAPPER= rg -N --no-heading "^(pub |pub\(super\) |pub\(crate\) )?(fn|struct|enum|impl|const|static|type|mod) " \
  C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/herdr/ \
  | sed 's/^[^:]*://' | sort > /tmp/sig-after.txt
diff /tmp/sig-before.txt /tmp/sig-after.txt
```

Task 1 records `sig-before.txt`. Every later task regenerates `sig-after.txt` and expects `diff` to report nothing, except for the lines a task legitimately adds (`mod x;` in `mod.rs`, and for Task 7 the items arriving from `app.rs`). Each task below states exactly which lines are allowed to differ.

**Check B, the test count.** The suite total must not move.

```sh
RUSTC_WRAPPER= cargo test -p alacritree 2>&1 | rg "test result"
```

Task 1 records the baseline count. Every later task expects the same number of passing tests and zero failures.

---

### Task 1: Establish the module baseline

Turn the half-finished reorganization into a compiling `src/herdr/mod.rs` that holds the whole original file, and record the two verification baselines. Nothing is split yet.

**Files:**
- Create: `alacritree/src/herdr/mod.rs` (from the content of `alacritree/src/herdr/herdr.rs`)
- Delete: `alacritree/src/herdr/herdr.rs`, `alacritree/src/herdr/app.rs`, `alacritree/src/herdr/herdr_e2e.rs`, `test.sock`, `test-client.sock`
- Already deleted by the user: `alacritree/src/herdr.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `crate::herdr` resolves to `src/herdr/mod.rs` and exports exactly what `src/herdr.rs` used to export. `/tmp/sig-before.txt` and the recorded baseline test count.

- [ ] **Step 1: Claim the files**

```sh
DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm acquire \
  C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/herdr/mod.rs \
  --note "herdr module split: baseline"
```

- [ ] **Step 2: Move the original file into place as `mod.rs`**

Overwrite the broken one-line `mod.rs` with the full content of `herdr/herdr.rs`, then remove the source and the empty placeholders.

```sh
R=C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette
cp "$R/alacritree/src/herdr/herdr.rs" "$R/alacritree/src/herdr/mod.rs"
rm "$R/alacritree/src/herdr/herdr.rs" "$R/alacritree/src/herdr/app.rs" "$R/alacritree/src/herdr/herdr_e2e.rs"
rm "$R/test.sock" "$R/test-client.sock"
```

The file's own module doc comment (`//! Surface agents running under a herdr server in the sidebar.` and the seven lines under it) stays exactly as written. It describes the module, which is still what this file is.

- [ ] **Step 3: Verify it compiles and the suite is unchanged**

```sh
RUSTC_WRAPPER= cargo check -p alacritree
RUSTC_WRAPPER= cargo test -p alacritree 2>&1 | rg "test result"
```

Expected: `cargo check` clean. Record the exact test result line in your report; it is the baseline for every later task. On this branch it was 1552 passing with 16 skipped at commit `fd4412f2`, but trust what you measure, not that number.

- [ ] **Step 4: Record the signature baseline**

```sh
RUSTC_WRAPPER= rg -N --no-heading "^(pub |pub\(super\) |pub\(crate\) )?(fn|struct|enum|impl|const|static|type|mod) " \
  C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/herdr/ \
  | sed 's/^[^:]*://' | sort > /tmp/sig-before.txt
wc -l /tmp/sig-before.txt
```

Expected: 68 lines. Also append the item signatures that Task 7 will move in from `app.rs`, so the final comparison balances:

```sh
RUSTC_WRAPPER= rg -N --no-heading "^(struct HerdrViewSync|enum HerdrViewAction|impl HerdrViewSync|fn needs_view_focus|fn herdr_attach_gesture|struct HerdrViewFocus|type HerdrAttachResult)" \
  C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/app.rs \
  | sed 's/^[^:]*://' >> /tmp/sig-before.txt
sort -o /tmp/sig-before.txt /tmp/sig-before.txt
```

- [ ] **Step 5: Format, lint, commit**

```sh
RUSTC_WRAPPER= devrun task fmt
RUSTC_WRAPPER= cargo clippy -p alacritree
R=C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette
git -C "$R" add alacritree/src/herdr.rs alacritree/src/herdr/mod.rs
git -C "$R" diff --staged --stat
```

Expected: git reports this as a rename of `alacritree/src/herdr.rs` to `alacritree/src/herdr/mod.rs` with no content change. If it reports adds and deletes instead, the copy was not byte-identical; redo Step 2.

```sh
git -C "$R" commit -m "$(cat <<'EOF'
refactor(herdr): move the module into its own directory

The file is about to be split into one file per concern.  Moving it
first keeps that split reviewable as a sequence of pure moves rather
than one diff that both relocates and rewrites.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Extract `model.rs`

Move the vocabulary out of `mod.rs`. This file must end up with no `serde` import: parsing lives in `wire.rs`, which Task 3 creates.

**Files:**
- Create: `alacritree/src/herdr/model.rs`
- Modify: `alacritree/src/herdr/mod.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `crate::herdr::{Side, Indicators, Settings, Status, Agent, Listing, HerdrKey, PollError, unattached, error_code, match_workspace}`, all re-exported from `mod.rs` so no call site outside the module changes.

Items to move, by their line numbers in `mod.rs` as it stands after Task 1:

| Lines | Item |
|---|---|
| 22-31 | `pub enum Side` |
| 32-40 | `pub enum Indicators` |
| 41-48 | `pub struct Settings` |
| 49-57 | `pub enum Status` |
| 58-84 | `impl Status` |
| 85-112 | `pub struct Agent` |
| 113-117 | `pub enum Listing` |
| 118-132 | `impl Listing`, the `wanted` and `args` methods only, plus the closing brace |
| 396-403 | `pub struct HerdrKey` |
| 404-412 | `pub fn unattached` |
| 413-426 | `pub fn error_code` |
| 427-442 | `pub fn match_workspace` |
| 443-464 | `fn starts_with` |
| 471-480 | `pub enum PollError` |
| 481-494 | `impl PollError` |

`impl Listing` splits. `wanted` maps a config flag to a listing and `args` names the herdr subcommand that produces it; both are facts about the enum, so they stay with it here as `impl Listing { wanted, args }`. `parse` (lines 134-149) is the JSON parser and goes to `wire.rs` in Task 3 as a second `impl Listing` block. An inherent impl may live in any module of the defining crate, so both halves stay reachable as `Listing::wanted`, `Listing::args` and `Listing::parse` with no re-export.

`fn starts_with` is used only by `match_workspace`, so it stays private with no visibility keyword.

- [ ] **Step 1: Claim the files**

```sh
DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm acquire \
  C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/herdr/model.rs \
  C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/herdr/mod.rs \
  --note "herdr module split: model"
```

- [ ] **Step 2: Create `model.rs` with the moved items**

Header of the new file:

```rust
//! What alacritree means by a herdr agent, and the pure questions it can ask
//! about one.

use std::path::{Path, PathBuf};

use crate::wsl;
```

Do not guess beyond that. Move the code, run `cargo check -p alacritree`, and add exactly the imports the compiler names. `std::path::{Path, PathBuf}` covers `match_workspace` and `starts_with`, and `crate::wsl` covers the WSL path translation inside `match_workspace`. This file must not import `serde`; if the compiler asks for it, something that belongs in `wire.rs` came along by mistake.

Move each item in the table verbatim, in the order listed, doc comments included.

- [ ] **Step 3: Move the matching tests**

These tests live in `mod tests` in `mod.rs` and move into a `#[cfg(test)] mod tests` at the bottom of `model.rs`:

| Lines | Test |
|---|---|
| 1779-1785 | `reads_the_error_code_off_stderr` |
| 1786-1794 | `status_label_names_each_variant` |
| 1995-2008 | `fn at` (helper) |
| 2009-2015 | `prefers_foreground_cwd_when_present` |
| 2016-2021 | `falls_back_to_cwd_when_foreground_is_absent` |
| 2022-2029 | `takes_the_longest_matching_prefix` |
| 2030-2035 | `a_sibling_with_a_shared_prefix_does_not_match` |
| 2036-2042 | `an_unmatched_agent_has_no_workspace` |
| 2043-2052 | `windows_prefixes_compare_case_insensitively` |
| 2053-2066 | `wsl_agent_matches_by_the_translated_windows_path` |
| 2067-2075 | `a_wsl_agent_outside_every_workspace_still_has_none` |
| 2076-2084 | `an_attached_agent_yields_no_row` |
| 2085-2092 | `detaching_brings_the_row_back` |
| 2093-2099 | `a_claim_on_one_side_does_not_hide_the_other_side` |

The moved test module opens with `use super::*;`, matching the one it came from.

Several of these build an `Agent` through the JSON parser. If a moved test calls `Listing::parse`, it now needs `use crate::herdr::Listing;` plus whatever `wire.rs` will provide, and `wire.rs` does not exist yet. Leave such a test in `mod.rs` for now and move it in Task 3 instead; note in your report which tests you deferred.

- [ ] **Step 4: Wire it up in `mod.rs`**

Delete the moved items from `mod.rs`, and add at the top, after the module doc comment:

```rust
mod model;

pub use model::{
    error_code, match_workspace, unattached, Agent, HerdrKey, Indicators, Listing, PollError,
    Settings, Side, Status,
};
```

- [ ] **Step 5: Verify**

```sh
RUSTC_WRAPPER= devrun task fmt
RUSTC_WRAPPER= cargo check -p alacritree
RUSTC_WRAPPER= cargo clippy -p alacritree
RUSTC_WRAPPER= cargo test -p alacritree 2>&1 | rg "test result"
```

Expected: the same test count as Task 1's baseline, zero failures, and no new clippy warnings.

```sh
RUSTC_WRAPPER= rg -N --no-heading "^(pub |pub\(super\) |pub\(crate\) )?(fn|struct|enum|impl|const|static|type|mod) " \
  C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/herdr/ \
  | sed 's/^[^:]*://' | sort > /tmp/sig-after.txt
diff /tmp/sig-before.txt /tmp/sig-after.txt
```

Expected: the only differences are lines Task 7 has not yet supplied (`struct HerdrViewSync`, `enum HerdrViewAction`, `impl HerdrViewSync`, `fn needs_view_focus`, `fn herdr_attach_gesture`, `struct HerdrViewFocus`, `type HerdrAttachResult`) plus the newly added `mod model;`. Nothing else.

- [ ] **Step 6: Commit**

```sh
R=C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette
git -C "$R" add alacritree/src/herdr/model.rs alacritree/src/herdr/mod.rs
git -C "$R" diff --staged
git -C "$R" commit -m "$(cat <<'EOF'
refactor(herdr): split the domain model into its own file

Separates what alacritree means by an agent from how herdr reports one,
so a reader can find the vocabulary without reading the parser.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Extract `wire.rs`

Move the serde types and the parsing that converts them into model types. After this task, `model.rs` has no `serde` dependency and `wire.rs` is the only place that knows herdr's JSON shapes.

**Files:**
- Create: `alacritree/src/herdr/wire.rs`
- Modify: `alacritree/src/herdr/mod.rs`

**Interfaces:**
- Consumes: `crate::herdr::{Agent, Listing, Status}` from Task 2.
- Produces: `impl Listing { pub fn parse(&self, stdout: &str) -> Vec<Agent> }` stays reachable as `Listing::parse` because an inherent impl in a sibling module of the same crate is still an inherent impl. `pub(super) struct SessionList` and `pub(super) struct RawSession` become available to `cli.rs` in Task 4.

Items to move, by their line numbers in `mod.rs` after Task 2's deletions. Re-derive them with `rg` rather than trusting arithmetic:

```sh
RUSTC_WRAPPER= rg -n "^(struct Envelope|struct Listed|struct RawPane|impl RawPane|impl Listing|struct SessionList|struct RawSession)" \
  C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/herdr/mod.rs
```

| Item | Original lines | New visibility |
|---|---|---|
| `impl Listing`, the `parse` method only | 134-149 | unchanged; `parse` stays `pub` |
| `struct Envelope` | 153-160 | private to `wire.rs` |
| `struct Listed` | 161-170 | private to `wire.rs` |
| `struct RawPane` | 171-183 | private to `wire.rs` |
| `impl RawPane` (`into_agent`) | 184-212 | unchanged, private |
| `struct SessionList` | 348-353 | `pub(super)` |
| `struct RawSession` | 354-365 | `pub(super)` |

`SessionList` and `RawSession` are used only by `running_session_name`, which goes to `cli.rs` in Task 4. They live here because they are JSON shapes, and `pub(super)` is what lets `cli.rs` reach them without widening the module's public surface.

Wrap the `parse` method in its own `impl Listing { ... }` block. Task 2 left `wanted` and `args` behind in `model.rs`, so `Listing` ends up with two inherent impl blocks in two files. That is legal and deliberate: the vocabulary and the parser are the two things this split exists to separate.

- [ ] **Step 1: Claim the files**

```sh
DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm acquire \
  C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/herdr/wire.rs \
  C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/herdr/mod.rs \
  --note "herdr module split: wire"
```

- [ ] **Step 2: Create `wire.rs`**

Header:

```rust
//! What herdr's CLI prints, and how it becomes an [`Agent`].
//!
//! herdr prints success on stdout and errors on stderr, and its field set
//! grows between releases, so every type here tolerates unknown fields and a
//! reply that carries none of what was asked for.

use serde::Deserialize;

use super::{Agent, Listing, Status};
```

Move the items verbatim. Adjust only the visibility keywords named in the table.

- [ ] **Step 3: Move the matching tests**

Into a `#[cfg(test)] mod tests` in `wire.rs`, opening with `use super::*;`:

| Original lines | Test |
|---|---|
| 1498-1507 | `an_agents_pane_title_is_parsed` |
| 1508-1516 | `a_titleless_agent_parses_with_no_title` |
| 1517-1526 | `a_blank_title_is_no_title` |
| 1527-1535 | `a_blank_kind_is_no_kind` |
| 1668-1685 | `parses_a_windows_agent_with_absent_optional_fields` |
| 1686-1692 | `parses_a_wsl_agent_with_foreground_cwd` |
| 1693-1698 | `empty_agent_list_is_not_an_error` |
| 1699-1708 | `unknown_fields_and_unknown_status_survive` |
| 1709-1718 | `an_agent_without_an_identity_is_dropped_alone` |
| 1719-1740 | `display_agent_wins_over_agent` |
| 1741-1754 | `a_pane_listing_keeps_the_shell_beside_the_agent` |
| 1755-1762 | `showing_panes_swaps_the_listing_rather_than_adding_one` |

Also move any test you deferred from Task 2 Step 3 because it needed the parser.

- [ ] **Step 4: Wire it up in `mod.rs`**

```rust
mod wire;
```

No `pub use` line: `wire.rs` exports nothing publicly. `Listing::parse` remains reachable through the `Listing` type itself.

- [ ] **Step 5: Verify**

Run the same four commands as Task 2 Step 5, then the signature diff. Expected: same test count, and the only new line versus Task 2 is `mod wire;`.

- [ ] **Step 6: Commit**

```sh
R=C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette
git -C "$R" add alacritree/src/herdr/wire.rs alacritree/src/herdr/mod.rs
git -C "$R" diff --staged
git -C "$R" commit -m "$(cat <<'EOF'
refactor(herdr): split the wire types into their own file

The serde types sat forty lines from the domain types they produce, so
telling herdr's reply apart from alacritree's model meant reading both.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Extract `cli.rs`

Move process invocation: argv construction, the bounded-spawn helper, and the commands that map one-to-one onto herdr subcommands.

**Files:**
- Create: `alacritree/src/herdr/cli.rs`
- Modify: `alacritree/src/herdr/mod.rs`

**Interfaces:**
- Consumes: `crate::herdr::{Agent, Side, Status}` from Task 2, and `pub(super) SessionList`/`RawSession` from Task 3.
- Produces: `crate::herdr::{attach_args, can_attach, attaches_directly, focus_args, focus_pane_args, focus_pane, running_session_name}` and the inherent `Side::command` and `Side::label`. `pub(super) fn bounded` becomes available to `settings.rs` in Task 5.

| Item | Original lines | New visibility |
|---|---|---|
| `fn sh_quote` | 213-219 | private |
| `impl Side` (`command`, `label`) | 220-251 | unchanged |
| `pub fn attach_args` | 252-258 | unchanged |
| `pub fn can_attach` | 259-274 | unchanged |
| `pub fn attaches_directly` | 275-283 | unchanged |
| `const GESTURE_TIMEOUT` | 284-290 | private |
| `fn bounded` | 291-305 | `pub(super)` |
| `pub fn focus_args` | 306-313 | unchanged |
| `pub fn focus_pane_args` | 314-323 | unchanged |
| `pub fn focus_pane` | 324-347 | unchanged |
| `pub fn running_session_name` | 366-395 | unchanged |
| `fn list_panes` | 1078-1106 | `pub(super)` |

`bounded` becomes `pub(super)` because `settings.rs` calls it in Task 5. `list_panes` becomes `pub(super)` because `poll.rs` calls it in Task 6. Both are internal to the herdr module and must not become `pub`.

`GESTURE_TIMEOUT` and its doc comment move together; the comment explains why three seconds, which is still true here.

- [ ] **Step 1: Claim the files**

```sh
DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm acquire \
  C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/herdr/cli.rs \
  C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/herdr/mod.rs \
  --note "herdr module split: cli"
```

- [ ] **Step 2: Create `cli.rs`**

Header:

```rust
//! Running the `herdr` binary.
//!
//! Everything goes through the CLI rather than herdr's socket, so a missing
//! binary or an absent server is a silent no-op and no wire protocol is
//! pinned.  This is the only file that knows how to reach a herdr server; a
//! second multiplexer would need its own equivalent and nothing else.

use std::process::Stdio;
use std::time::Duration;

use crate::config::AttachMode;
use crate::{command_ext, jobs, wsl};

use super::wire::{RawSession, SessionList};
use super::{Agent, PollError, Side, Status};
```

Do not guess the import list. Write the moved code first, run `cargo check -p alacritree`, and add exactly what it names. `list_panes` in particular may pull in `jobs` and `PollError`; confirm from the compiler.

- [ ] **Step 3: Move the matching tests**

| Original lines | Test |
|---|---|
| 1763-1771 | `a_pane_with_no_agent_is_focused_through_its_tab` |
| 1772-1778 | `a_pane_with_no_agent_never_attaches_directly` |
| 1795-1800 | `native_windows_cannot_attach_directly` |
| 1801-1807 | `wsl_can_always_attach` |
| 1808-1816 | `asking_for_the_session_gives_up_a_direct_attach` |
| 1817-1825 | `native_runs_herdr_directly` |
| 1826-1832 | `wsl_wraps_in_a_login_shell` |
| 1833-1838 | `wsl_quotes_arguments_that_need_it` |
| 1839-1843 | `direct_attach_targets_the_pane_id` |

- [ ] **Step 4: Wire it up in `mod.rs`**

```rust
mod cli;

pub use cli::{
    attach_args, attaches_directly, can_attach, focus_args, focus_pane, focus_pane_args,
    running_session_name,
};
```

`Side::command` and `Side::label` need no re-export; they are inherent methods on a type `mod.rs` already exports.

- [ ] **Step 5: Verify**

Same four commands, then the signature diff. Expected: same test count, and the only new line versus Task 3 is `mod cli;`.

- [ ] **Step 6: Commit**

```sh
R=C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette
git -C "$R" add alacritree/src/herdr/cli.rs alacritree/src/herdr/mod.rs
git -C "$R" diff --staged
git -C "$R" commit -m "$(cat <<'EOF'
refactor(herdr): split process invocation into its own file

Collects every argv and every spawn in one place, which is the surface a
second multiplexer would have to supply.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: Extract `settings.rs`

Move reading and interpreting herdr's own `config.toml`. The spawn that fetches the file comes along, because that path exists only to produce a `Settings`.

The file is named `settings.rs`, not `config.rs`, because the crate already has a `config.rs` meaning alacritree's own configuration, and two files with that name meaning different configurations is exactly the confusion this split exists to remove.

**Files:**
- Create: `alacritree/src/herdr/settings.rs`
- Modify: `alacritree/src/herdr/mod.rs`

**Interfaces:**
- Consumes: `crate::herdr::{Indicators, Settings, Side}` from Task 2 and `pub(super) fn bounded` from Task 4.
- Produces: `crate::herdr::settings(side, blocking) -> Option<Settings>`.

| Item | Original lines | New visibility |
|---|---|---|
| `const DEFAULT_PREFIX`, `const DEFAULT_DETACH` | 1107-1110 | private |
| `struct RawHerdrConfig` | 1111-1118 | private |
| `struct RawUi` | 1119-1123 | private |
| `struct RawKeys` | 1124-1131 | private |
| `enum RawBinding` | 1132-1136 | private |
| `impl RawBinding` (`first`) | 1137-1152 | private |
| `fn render_combo` | 1153-1156 | private |
| `fn render_key` | 1157-1168 | private |
| `fn render_prefixed` | 1169-1178 | private |
| `fn detach_chord_from` | 1179-1197 | private |
| `fn indicators_from` | 1198-1205 | private |
| `fn settings_from` | 1206-1213 | private |
| `fn native_config_path` | 1214-1232 | private |
| `const CONFIG_SCRIPT` | 1233-1240 | private |
| `fn read_config` | 1241-1265 | private |
| `pub fn settings` | 1266-1272 | unchanged |

- [ ] **Step 1: Claim the files**

```sh
DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm acquire \
  C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/herdr/settings.rs \
  C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/herdr/mod.rs \
  --note "herdr module split: settings"
```

- [ ] **Step 2: Create `settings.rs`**

Header:

```rust
//! Reading herdr's own configuration, so alacritree can name the chord that
//! detaches a session and draw the indicator set herdr draws.

use std::path::PathBuf;

use crate::jobs;

use super::cli::bounded;
use super::{Indicators, Settings, Side};
```

Again, let `cargo check` settle the import list rather than trusting this header.

- [ ] **Step 3: Move the matching tests**

| Original lines | Test |
|---|---|
| 1536-1547 | `an_untouched_config_yields_herdrs_documented_chord` |
| 1548-1555 | `a_rebound_prefix_moves_the_first_half` |
| 1556-1565 | `a_rebound_detach_moves_the_second_half` |
| 1566-1575 | `a_direct_detach_binding_drops_the_prefix` |
| 1576-1585 | `a_list_of_bindings_renders_the_first` |
| 1586-1607 | `an_unbound_detach_has_no_chord` |
| 1608-1614 | `an_unparseable_config_falls_back_to_the_defaults` |
| 1615-1637 | `the_indicator_set_follows_herdrs_own_choice` |
| 1638-1645 | `an_unknown_indicator_set_keeps_the_shipped_one` |
| 1646-1667 | `settings_carry_both_halves_of_the_config` |

- [ ] **Step 4: Wire it up in `mod.rs`**

```rust
mod settings;

pub use settings::settings;
```

- [ ] **Step 5: Verify**

Same four commands, then the signature diff. Expected: same test count, and the only new line versus Task 4 is `mod settings;`.

- [ ] **Step 6: Commit**

```sh
R=C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette
git -C "$R" add alacritree/src/herdr/settings.rs alacritree/src/herdr/mod.rs
git -C "$R" diff --staged
git -C "$R" commit -m "$(cat <<'EOF'
refactor(herdr): split herdr's own config reading into a file

Named for the Settings it produces rather than for config, since the
crate already has a config module meaning alacritree's own.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: Extract `poll.rs`

Move endpoint reachability, caching, and poll scheduling. This is the largest file at roughly 640 lines, and after it `mod.rs` holds no code of its own.

**Files:**
- Create: `alacritree/src/herdr/poll.rs`
- Modify: `alacritree/src/herdr/mod.rs`

**Interfaces:**
- Consumes: everything from Tasks 2 through 5, notably `pub(super) fn list_panes` from Task 4 and `crate::herdr::settings` from Task 5.
- Produces: `crate::herdr::{Reach, EndpointCache, Endpoints, PaneInventory, PaneMetadata}`.

| Item | Original lines | New visibility |
|---|---|---|
| `const RECOVERY_RETRY` | 465-470 | private |
| `pub struct Reach` | 495-502 | unchanged |
| `impl Reach` | 503-535 | unchanged |
| `struct ListingReply` | 536-542 | private |
| `impl ListingReply` | 543-555 | private |
| `pub struct PaneInventory` | 556-560 | unchanged |
| `fn parse_pane_inventory` | 561-585 | private |
| `pub struct PaneMetadata` | 586-591 | unchanged |
| `pub struct EndpointCache` | 592-605 | unchanged |
| `impl EndpointCache` | 606-909 | unchanged |
| `const DISTRO_REFRESH` | 910-913 | private |
| `const MEMBERSHIP_SCALE` | 914-919 | private |
| `pub struct Endpoints` | 920-931 | unchanged |
| `impl Default for Endpoints` | 932-943 | unchanged |
| `impl Endpoints` | 944-1054 | unchanged |
| `fn rendered_differs` | 1055-1077 | private |
| `enum Read<T>` | 1273-1279 | private |

`EndpointCache` carries test-only methods (`caches_mut_for_test`, `complete_listing_for_test`) used from `app.rs`'s test module. Those move with the impl block and must keep whatever visibility they have, or `app.rs`'s tests stop compiling.

- [ ] **Step 1: Claim the files**

```sh
DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm acquire \
  C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/herdr/poll.rs \
  C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/herdr/mod.rs \
  --note "herdr module split: poll"
```

- [ ] **Step 2: Create `poll.rs`**

Header:

```rust
//! Asking each reachable herdr server what it has, on a schedule that backs
//! off a side that never answers and recovers one that starts answering
//! again.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::{jobs, wsl};

use super::cli::list_panes;
use super::{Agent, Listing, PollError, Settings, Side};
```

- [ ] **Step 3: Move the matching tests**

| Original lines | Test |
|---|---|
| 1283-1313 | `attachment_metadata_survives_a_first_poll_failure_after_binding` |
| 1314-1328 | `attachment_metadata_is_released_without_bindings` |
| 1329-1356 | `inventory_adoption_keeps_the_request_timestamp_when_display_changes_in_flight` |
| 1357-1379 | `attached_inventory_retries_failed_and_malformed_polls_at_the_configured_interval` |
| 1380-1400 | `failed_inventory_jobs_invalidate_success_and_keep_attached_retries_alive` |
| 1401-1420 | `inventory_unchanged_frames_do_not_request_immediate_polls` |
| 1421-1439 | `pane_display_without_attachments_does_not_build_an_inventory` |
| 1440-1460 | `failed_distro_listing_invalidates_only_wsl_inventory_evidence` |
| 1461-1497 | `unchanged_listings_advance_freshness_without_rebuilding_rows` |
| 1844-1855 | the `use std::time::Duration;` line and `an_endpoint_with_no_herdr_is_given_up_on` |
| 1856-1867 | `a_server_that_starts_later_is_still_found` |
| 1868-1875 | `a_side_that_loses_its_herdr_stops_being_polled` |
| 1876-1884 | `an_endpoint_that_answered_once_keeps_retrying` |
| 1885-1893 | `a_recovered_endpoint_polls_at_the_normal_interval_again` |
| 1894-1909 | `an_endpoint_is_abandoned_only_after_a_failure_it_never_answered` |
| 1910-1931 | `an_endpoint_follows_its_distro_starting_and_stopping` |
| 1932-1945 | `removing_an_endpoint_that_landed_a_poll_is_still_observable` |
| 1946-1959 | `a_failed_listing_leaves_the_endpoint_set_alone` |
| 1960-1967 | `a_repeated_error_is_logged_once` |
| 1968-1981 | `fn agent` (helper) |
| 1982-1987 | `an_unchanged_agent_list_is_not_a_change` |
| 1988-1994 | `a_status_change_counts` |

- [ ] **Step 4: Wire it up in `mod.rs`**

```rust
mod poll;

pub use poll::{EndpointCache, Endpoints, PaneInventory, PaneMetadata, Reach};
```

- [ ] **Step 5: Verify**

Same four commands, then the signature diff. Expected: same test count, and the only new line versus Task 5 is `mod poll;`.

At this point `mod.rs` should contain only the module doc comment, six `mod` declarations, and the `pub use` blocks. Confirm it:

```sh
RUSTC_WRAPPER= rg -c "^(fn|struct|enum|impl|const|static|type) " \
  C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/herdr/mod.rs
```

Expected: no matches, so `rg` exits 1 and prints nothing.

- [ ] **Step 6: Commit**

```sh
R=C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette
git -C "$R" add alacritree/src/herdr/poll.rs alacritree/src/herdr/mod.rs
git -C "$R" diff --staged
git -C "$R" commit -m "$(cat <<'EOF'
refactor(herdr): split endpoint polling into its own file

Leaves mod.rs holding only declarations and re-exports, so the module's
whole public surface fits on one screen.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 7: Move the herdr-shaped code out of `app.rs`

This is the only task that touches `app.rs`. It moves the focus-reconciliation model into a new `view.rs` and the attach gesture into the existing `cli.rs`.

**Files:**
- Create: `alacritree/src/herdr/view.rs`
- Modify: `alacritree/src/herdr/cli.rs`, `alacritree/src/herdr/mod.rs`, `alacritree/src/app.rs`

**Interfaces:**
- Consumes: `crate::herdr::{Agent, HerdrKey, Side, attaches_directly, focus_pane, running_session_name}`, plus `crate::session::SessionId` and `crate::config::AttachMode`.
- Produces: `crate::herdr::{HerdrViewSync, HerdrViewAction, HerdrViewFocus, needs_view_focus}` and `crate::herdr::{HerdrAttachResult, herdr_attach_gesture}`.

To `view.rs`:

| Item | `app.rs` lines, doc comments and attributes included |
|---|---|
| `struct HerdrViewFocus` | 9254-9260 |
| `struct HerdrViewSync` | 9262-9267 |
| `enum HerdrViewAction` | 9269-9273 |
| `impl HerdrViewSync` | 9275-9333 |
| `fn needs_view_focus` | 9335-9347 |

To `cli.rs`:

| Item | `app.rs` lines, doc comments included |
|---|---|
| `type HerdrAttachResult` | 9238 |
| `fn herdr_attach_gesture` | 9349-9410 |

Everything else herdr-shaped stays in `app.rs` for now. `PendingHerdrAttach` (9240-9252) holds a `WorkspaceKey` and stays. The `&mut AlacritreeApp` methods at 1559-1822 and the accessor cluster at 9605-9800 stay. They manipulate app state, and moving them is a later, larger decision about whether `herdr/` may depend on the presentation layer.

These items are currently private to `app.rs` with no visibility keyword. They become `pub` on the way out, because `app.rs` is a different module once they move. That is a visibility change, which the pure move rule allows.

- [ ] **Step 1: Claim the files**

```sh
DEVKIT_SESSION=$CLAUDE_CODE_SESSION_ID lockm acquire \
  C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/herdr/view.rs \
  C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/herdr/cli.rs \
  C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/herdr/mod.rs \
  C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/app.rs \
  --note "herdr module split: view and attach gesture out of app.rs"
```

`app.rs` is the file other agents are most likely to be holding. If the claim is refused, stop and report; do not force-release a lock another session owns.

- [ ] **Step 2: Create `view.rs`**

Header:

```rust
//! Which herdr pane alacritree is showing, and when to tell herdr to move.
//!
//! Every herdr client draws the one pane herdr has focused, so a session
//! sharing herdr's view has to ask for its own pane before its client can
//! start, and follows herdr afterwards.

use std::time::Instant;

use crate::config::AttachMode;
use crate::session::SessionId;

use super::{attaches_directly, Agent, HerdrKey, Side};
```

`HerdrViewFocus` holds a `jobs::Job`, so `use crate::jobs;` comes too. Let `cargo check` confirm.

- [ ] **Step 3: Move the view tests out of `app.rs`**

Into a `#[cfg(test)] mod tests` in `view.rs`. These test `HerdrViewSync::next` and `needs_view_focus` directly and need no `AlacritreeApp`:

| `app.rs` lines | Test |
|---|---|
| 12549-12582 | `herdr_shared_view_follows_new_tabs_and_refocuses_on_return` |
| 12583-12594 | `herdr_shared_view_refocuses_after_an_ordinary_session` |
| 12595-12623 | `herdr_follow_attempts_wait_for_a_new_snapshot` |
| 12624-12668 | `herdr_shared_view_rejects_stale_and_foreign_focus_snapshots` |
| 12669-12690 | `herdr_focus_completion_cannot_restore_a_view_left_while_pending` |
| 13909-13918 | `a_shared_view_asks_herdr_for_its_pane` |
| 13919-13935 | `a_direct_attach_never_asks_herdr_for_its_pane` |
| 13936-13945 | `a_shared_view_asks_once_per_switch` |

`herdr_focus_completion_for_a_removed_session_is_ignored` at 12536-12548 does **not** move. It calls `app.sync_herdr_view_focus`, so it stays in `app.rs`.

Verify the exact end line of each test before cutting, since the ranges above are read off item starts:

```sh
RUSTC_WRAPPER= rg -n "fn herdr_shared_view_follows_new_tabs_and_refocuses_on_return" -A 40 \
  C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/app.rs
```

- [ ] **Step 4: Move the gesture into `cli.rs`**

Append `type HerdrAttachResult` and `fn herdr_attach_gesture` to `cli.rs`, both `pub`. Its doc comment explains why both calls run on the pool and moves with it unchanged.

- [ ] **Step 5: Wire it up in `mod.rs`**

```rust
mod view;

pub use cli::{herdr_attach_gesture, HerdrAttachResult};
pub use view::{needs_view_focus, HerdrViewAction, HerdrViewFocus, HerdrViewSync};
```

Merge the `cli` line into the existing `pub use cli::{...}` block rather than adding a second one.

- [ ] **Step 6: Fix the call sites in `app.rs`**

`app.rs` already has `use crate::herdr;` and refers to herdr items as `herdr::Foo`. The moved items were bare names, so every reference needs the `herdr::` prefix. Find them:

```sh
RUSTC_WRAPPER= rg -n "HerdrViewSync|HerdrViewAction|HerdrViewFocus|needs_view_focus|herdr_attach_gesture|HerdrAttachResult" \
  C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/app.rs
```

Prefixing each is the minimal change and keeps `app.rs` reading the way it already does for `herdr::HerdrKey`. Do not add a `use crate::herdr::{...}` block that imports them bare; that would make two spellings of the same thing coexist in one file.

- [ ] **Step 7: Verify**

```sh
RUSTC_WRAPPER= devrun task fmt
RUSTC_WRAPPER= cargo check -p alacritree
RUSTC_WRAPPER= cargo clippy -p alacritree
RUSTC_WRAPPER= cargo test -p alacritree 2>&1 | rg "test result"
```

Expected: the same test count as Task 1's baseline, zero failures, no new clippy warnings.

```sh
RUSTC_WRAPPER= rg -N --no-heading "^(pub |pub\(super\) |pub\(crate\) )?(fn|struct|enum|impl|const|static|type|mod) " \
  C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/herdr/ \
  | sed 's/^[^:]*://' | sort > /tmp/sig-after.txt
diff /tmp/sig-before.txt /tmp/sig-after.txt
```

Expected: the only differences are the six `mod` lines added across Tasks 2 to 7, and the `pub ` prefix now carried by the seven items that arrived from `app.rs`. Every other line matches. If an item appears in one file and not the other, something was dropped or duplicated; find it before committing.

- [ ] **Step 8: Commit**

```sh
R=C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette
git -C "$R" add alacritree/src/herdr/view.rs alacritree/src/herdr/cli.rs \
  alacritree/src/herdr/mod.rs alacritree/src/app.rs
git -C "$R" diff --staged
git -C "$R" commit -m "$(cat <<'EOF'
refactor(herdr): move focus reconciliation out of app.rs

The view sync model and the attach gesture depend on no app state, so
they belong beside the rest of the herdr code rather than in the middle
of the app module.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 8: Confirm the split holds

No code changes. This task exists so a reviewer can accept the whole sequence on evidence rather than by reading two thousand moved lines.

**Files:**
- Modify: none.

**Interfaces:**
- Consumes: everything from Tasks 1 through 7.
- Produces: the report that closes the plan.

- [ ] **Step 1: Confirm the module shape**

```sh
wc -l C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/herdr/*.rs
```

Expected: seven files. `mod.rs` under 60 lines, `poll.rs` the largest. If any file other than `poll.rs` is over 700 lines, say so in your report; it means a concern was mis-assigned.

- [ ] **Step 2: Confirm no file grew a second responsibility**

```sh
RUSTC_WRAPPER= rg -l "serde" C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/herdr/
RUSTC_WRAPPER= rg -l "command_ext|Stdio" C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette/alacritree/src/herdr/
```

Expected: `serde` appears in `wire.rs` and `settings.rs` only. `command_ext` or `Stdio` appears in `cli.rs` only. Any other file matching means process spawning or wire parsing leaked, and the report must name it.

- [ ] **Step 3: Confirm the whole sequence is behaviour-neutral**

```sh
R=C:/Users/Lev/Git/github/alacritree-worktrees/feat/herdr-palette
git -C "$R" diff --stat fd4412f2 -- alacritree/src/
RUSTC_WRAPPER= cargo test -p alacritree 2>&1 | rg "test result"
```

Expected: the diffstat's added and deleted line counts are close to equal, differing only by the import and re-export lines the split introduced. A net gain of more than about 150 lines means something was rewritten rather than moved; find it and say so.

- [ ] **Step 4: Report**

State in your report: the final line count per file, the baseline and final test counts, the signature diff result, and any item whose placement you had to judge rather than read off this plan. Do not commit anything.

---

## Notes for the reviewer

The split is behaviour-neutral by construction, and its two verification checks are mechanical. What is worth a human's attention is placement rather than correctness:

- `impl Listing` ends up split across `model.rs` (`wanted`, `args`) and `wire.rs` (`parse`). Two inherent impl blocks for one type in two files is unusual, and keeping all three together in `model.rs` at the cost of a `serde` import there is defensible.
- `SessionList` and `RawSession` sit in `wire.rs` though only `cli.rs` uses them, on the grounds that they are JSON shapes. Moving them to `cli.rs` instead is defensible.
- `read_config` spawns a herdr process but lives in `settings.rs` rather than `cli.rs`, because that spawn exists only to produce a `Settings` and would not transfer to another multiplexer.
- `view.rs` keeps the `HerdrView*` naming rather than being called `focus.rs`, so the file matches the types in it. Renaming the types is out of scope for a pure move and would be a reasonable follow-up.
- `HerdrViewFocus` moved with the rest of the view model even though it holds a `SessionId` and a job handle, which are app-side concepts. Leaving it in `app.rs` is defensible.

The presentation helpers in `app.rs` (`herdr_cwd`, `herdr_subtitle`, `herdr_palette_content`, `herdr_row_name`, `herdr_display_name`, `herdr_mark`, `herdr_row`, `HerdrRowData`) are deliberately untouched. They are extractable, but they depend on `PaletteSessionContent`, `RowName`, `PathStyle`, `Icons` and `Theme`, so moving them forces a decision about whether `herdr/` may depend on the presentation layer. That is its own piece of work.
