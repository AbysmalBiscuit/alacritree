# Sidebar navigation implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** removing the workspace you are looking at lands on the session next to it, the sidebar scrolls to the session you navigate to, and a scrolled-to row can be centred, each behind a config key defaulting to today's behaviour.

**Architecture:** three independent features in one crate. The landing policy adds two values to the existing `ui.last_session_close` enum and resolves them through a new pure function over a snapshot of the session ring, consumed by both removal paths (`close_session` and `run_pending_delete`). The follow-scroll adds a per-frame comparison in the projects panel and a pure target-resolution function. The alignment adds one field to `Theme` and threads it into the five existing `scroll_to_rect` calls.

**Tech Stack:** Rust 2024 edition, MSRV 1.85. egui 0.31.1 / eframe. Tests are in-module `#[cfg(test)]` and run with `cargo nextest run -p alacritree`.

**Spec:** `docs/superpowers/specs/2026-09-05-sidebar-navigation-design.md`

## Global Constraints

- Every new behaviour is config-gated and defaults to today's behaviour. A user who changes nothing sees no difference. This is not negotiable: a second user runs this fork.
- Only `alacritree/` is edited. `alacritty/`, `alacritty_terminal/`, `alacritty_config/`, `alacritty_config_derive/` and `egui-winit/` are vendored read-only.
- Comments explain *why*, never restate *what*, and are timeless: no PR or issue references, no "previously", no "now we", no change narration, no RED/GREEN test narration in non-test code.
- Every new config field gets a doc comment on its `RawUi` field. Those doc comments are the hover text the published JSON schema carries.
- After any config change, regenerate the schema: `ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema`. The build fails while `schema/alacritree-config.json` is stale.
- `cargo fmt` is enforced. Run it before every commit.
- Conventional Commits, imperative mood, subject under 72 characters.
- Every commit carries the trailer `Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>`.
- Verification command for every task: `cargo nextest run -p alacritree`. Not `cargo test`.

Tasks 1 through 4 implement issue #63, task 5 implements #64, task 6 implements #65. The spec groups the work into three commits by issue; this plan commits per task, which is finer and matches the repo's one-logical-change-per-commit rule.

---

### Task 1: `project_of`, the shared project lookup

`project_main_for` already opens with this lookup and then asks a further question of the answer. Extracting it gives the ring policy and the session-reorder spec one definition of which project owns a workspace, and gives this task a live consumer so nothing is dead code.

**Files:**
- Modify: `alacritree/src/sidebar_nav.rs` (add `project_of` and its tests)
- Modify: `alacritree/src/app.rs:6376-6380` (`project_main_for` becomes a caller)

**Interfaces:**
- Consumes: nothing.
- Produces: `pub fn project_of(projects: &[Project], ws: &WorkspaceKey) -> Option<&Path>`.

- [ ] **Step 1: Write the failing tests**

Append to the `#[cfg(test)] mod tests` block at the end of `alacritree/src/sidebar_nav.rs`. Check the existing fixture helpers in that block first and reuse them if they already build a `Project`; the code below builds its own if they do not.

```rust
    fn wt_at(path: &str, is_main: bool) -> Worktree {
        Worktree {
            name: path.rsplit('/').next().unwrap_or(path).to_string(),
            path: PathBuf::from(path),
            branch: None,
            is_main,
            prunable: false,
            upstream: None,
        }
    }

    fn project_at(root: &str, worktrees: Vec<Worktree>) -> Project {
        Project {
            root: PathBuf::from(root),
            name: root.rsplit('/').next().unwrap_or(root).to_string(),
            label: None,
            default_branch: None,
            worktrees,
            expanded: true,
            shell_override: None,
            home: None,
        }
    }

    #[test]
    fn project_of_finds_the_owner_of_a_worktree() {
        let projects = vec![project_at("/p1", vec![wt_at("/p1", true), wt_at("/p1-wt/a", false)])];
        assert_eq!(
            project_of(&projects, &Some(PathBuf::from("/p1-wt/a"))),
            Some(Path::new("/p1"))
        );
    }

    #[test]
    fn home_and_unlisted_paths_belong_to_no_project() {
        let projects = vec![project_at("/p1", vec![wt_at("/p1", true)])];
        assert_eq!(project_of(&projects, &None), None);
        assert_eq!(project_of(&projects, &Some(PathBuf::from("/elsewhere"))), None);
    }

    /// git lets two projects list the same path.  The session records a
    /// directory, not a project, so sidebar order is the only tiebreak
    /// available and the first listing owns it.
    #[test]
    fn a_path_two_projects_list_belongs_to_the_first() {
        let shared = "/shared/wt";
        let projects = vec![
            project_at("/p1", vec![wt_at("/p1", true), wt_at(shared, false)]),
            project_at("/p2", vec![wt_at("/p2", true), wt_at(shared, false)]),
        ];
        assert_eq!(
            project_of(&projects, &Some(PathBuf::from(shared))),
            Some(Path::new("/p1"))
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```sh
cargo nextest run -p alacritree project_of
```

Expected: compilation failure, `cannot find function 'project_of' in this scope`.

- [ ] **Step 3: Write the implementation**

Add to `alacritree/src/sidebar_nav.rs`, after `visible_rows`. The imports it needs (`Path`, `PathBuf`, `Project`, `Worktree`, `WorkspaceKey`) are already at the top of the file.

```rust
/// The project whose worktree list contains `ws`, or None for home and for a
/// workspace no listed project owns.  A path two projects both list belongs to
/// the first in sidebar order; the session records a directory, not a project,
/// so nothing better is available.
pub fn project_of<'a>(projects: &'a [Project], ws: &WorkspaceKey) -> Option<&'a Path> {
    let path = ws.as_deref()?;
    projects
        .iter()
        .find(|p| p.worktrees.iter().any(|w| w.path == path))
        .map(|p| p.root.as_path())
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```sh
cargo nextest run -p alacritree project_of
```

Expected: 3 passed.

- [ ] **Step 5: Make `project_main_for` a caller**

Replace the body of `project_main_for` in `alacritree/src/app.rs` (currently at 6376). Keep its existing doc comment unchanged.

```rust
fn project_main_for(projects: &[Project], ws: &Path) -> Option<PathBuf> {
    let root = sidebar_nav::project_of(projects, &Some(ws.to_path_buf()))?;
    let project = projects.iter().find(|p| p.root == root)?;
    let main = project.worktrees.iter().find(|w| w.is_main)?;
    if main.path == ws { None } else { Some(main.path.clone()) }
}
```

`app.rs:46` already reads `use crate::sidebar_nav::{self, SidebarRow};`, so the module path above works with no import change.

- [ ] **Step 6: Verify the whole suite still passes**

```sh
cargo fmt
cargo nextest run -p alacritree
```

Expected: no failures. `project_main_for`'s existing tests, if any, still pass unchanged.

- [ ] **Step 7: Commit**

```sh
git add alacritree/src/sidebar_nav.rs alacritree/src/app.rs
git commit -m "refactor(sidebar): extract project_of from project_main_for

The ring close policy and the sidebar reorder model both need to know
which project owns a workspace.  One definition, in the pure sidebar
model, keeps them from disagreeing about a path two projects list.

Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>"
```

---

### Task 2: the two new `last_session_close` values

Config only. After this task the two values parse and are accepted, and behave exactly like `navigate` because `close_session` only special-cases `Respawn`. That is a safe intermediate state: no user reaches it without opting in, and the value they opted into does something reasonable.

**Files:**
- Modify: `alacritree/src/config.rs:586-606` (enum and parser), `:1953-1956` (`RawUi` doc comment and schema enum), `:2741-2760` (tests)
- Modify: `schema/alacritree-config.json` (regenerated, not hand-edited)
- Modify: `docs/alacritree.md`, `docs/keyboard-shortcuts.md`

**Interfaces:**
- Consumes: nothing.
- Produces: `LastSessionClose::RingGlobal` and `LastSessionClose::RingProject`, plus `LastSessionClose::rings(self) -> bool`.

- [ ] **Step 1: Write the failing tests**

Modify the two existing tests and add one, in `alacritree/src/config.rs`'s test module.

```rust
    #[test]
    fn last_session_close_parses_all_values() {
        for (raw, expected) in [
            ("respawn", LastSessionClose::Respawn),
            ("navigate", LastSessionClose::Navigate),
            ("ring_global", LastSessionClose::RingGlobal),
            ("ring_project", LastSessionClose::RingProject),
        ] {
            let ui = ui_from_toml(&format!("[ui]\nlast_session_close = \"{raw}\""));
            assert_eq!(ui.last_session_close, expected, "value {raw:?}");
        }
    }

    #[test]
    fn only_the_ring_values_ring() {
        assert!(!LastSessionClose::Respawn.rings());
        assert!(!LastSessionClose::Navigate.rings());
        assert!(LastSessionClose::RingGlobal.rings());
        assert!(LastSessionClose::RingProject.rings());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```sh
cargo nextest run -p alacritree last_session_close only_the_ring_values_ring
```

Expected: compilation failure, `no variant named 'RingGlobal'`.

- [ ] **Step 3: Extend the enum and the parser**

In `alacritree/src/config.rs`, replace the `LastSessionClose` enum and `parse_last_session_close`.

```rust
/// What happens when the on-screen workspace stops having sessions, whether a
/// close or a worktree deletion took the last one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LastSessionClose {
    /// Recycle a shell in place — the workspace always has a live session,
    /// so the last session is by design unclosable.
    #[default]
    Respawn,
    /// Move to the project's main checkout when it has a live session,
    /// otherwise home (which spawns a shell only if it has none).
    Navigate,
    /// Move to the nearest session in the flat session ring, otherwise home.
    RingGlobal,
    /// Move to the nearest session in the removed workspace's own project,
    /// then to the nearest anywhere in the ring, otherwise home.
    RingProject,
}

impl LastSessionClose {
    /// Whether the destination comes from the session ring.  Both removal
    /// paths build that ring only when this is true, so the default costs
    /// no allocation.
    pub fn rings(self) -> bool {
        matches!(self, Self::RingGlobal | Self::RingProject)
    }

    /// Whether the search is confined to the removed workspace's project
    /// before it widens to the whole ring.
    pub fn prefers_project(self) -> bool {
        matches!(self, Self::RingProject)
    }
}

fn parse_last_session_close(raw: Option<&str>) -> LastSessionClose {
    match raw {
        None => LastSessionClose::default(),
        Some("respawn") => LastSessionClose::Respawn,
        Some("navigate") => LastSessionClose::Navigate,
        Some("ring_global") => LastSessionClose::RingGlobal,
        Some("ring_project") => LastSessionClose::RingProject,
        Some(other) => {
            log::warn!("unknown ui.last_session_close value {other:?}, using \"respawn\"");
            LastSessionClose::default()
        },
    }
}
```

- [ ] **Step 4: Update the `RawUi` doc comment and schema enum**

In `alacritree/src/config.rs` at the `last_session_close` field of `RawUi` (currently 1953):

```rust
    /// What happens when the on-screen workspace stops having sessions,
    /// whether a close or a worktree deletion took the last one:
    /// "respawn" (default) | "navigate" | "ring_global" | "ring_project".
    #[schemars(extend("enum" = ["respawn", "navigate", "ring_global", "ring_project"]))]
    last_session_close: Option<String>,
```

- [ ] **Step 5: Run the tests to verify they pass**

```sh
cargo nextest run -p alacritree last_session_close only_the_ring_values_ring
```

Expected: 4 passed (the two modified, the new one, and `last_session_close_defaults_to_respawn`).

- [ ] **Step 6: Regenerate the schema**

```sh
ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema
```

Then confirm it is no longer stale:

```sh
cargo test -p alacritree --test config_schema
```

Expected: PASS. `git diff schema/alacritree-config.json` shows the two new enum members and the new description.

- [ ] **Step 7: Update the docs**

In `docs/alacritree.md`, find the annotated `[ui]` block's `last_session_close` line (near 414) and extend the comment to name all four values. In `docs/keyboard-shortcuts.md` (near 159), extend the sentence describing what `last_session_close` decides so it covers the ring values and says they also govern worktree deletion.

Both files are soft-wrapped prose: one line per paragraph, one line per bullet. Do not hard-wrap at a column.

- [ ] **Step 8: Verify and commit**

```sh
cargo fmt
cargo nextest run -p alacritree
git add alacritree/src/config.rs schema/alacritree-config.json docs/alacritree.md docs/keyboard-shortcuts.md
git commit -m "feat(config): add the ring values to last_session_close

Both parse and are documented; the removal paths that read them land in
the following commits, so today they behave as navigate does.

Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>"
```

---

### Task 3: the ring landing, wired into `close_session`

**Files:**
- Modify: `alacritree/src/app.rs` — add `RingEntry`, `ring_landing`, `workspace_order_with_projects`, `session_ring`; extend `CloseFallback`; rewrite part of `close_session` and `apply_close_fallback`; add tests

**Interfaces:**
- Consumes: `sidebar_nav::project_of` (Task 1), `LastSessionClose::{rings, prefers_project}` (Task 2).
- Produces:
  - `struct RingEntry { project: Option<PathBuf>, workspace: WorkspaceKey, id: SessionId }`
  - `fn ring_landing(ring: &[RingEntry], removed: &[SessionId], prefer: Option<&Path>) -> Option<(WorkspaceKey, SessionId)>`
  - `AlacritreeApp::session_ring(&self) -> Vec<RingEntry>`
  - `CloseFallback::ActivateSession(SessionId)`

- [ ] **Step 1: Write the failing tests**

Add to `alacritree/src/app.rs`'s `mod tests`. The `ws` helper already exists there.

```rust
    fn entry(project: Option<&str>, workspace: &str, id: SessionId) -> RingEntry {
        RingEntry {
            project: project.map(PathBuf::from),
            workspace: ws(workspace),
            id,
        }
    }

    /// The tree from the spec: home holds nothing, p1 owns w1 and w2, p2 owns
    /// w3.  Ring order is sidebar order, so p1's sessions precede p2's.
    fn spec_ring() -> Vec<RingEntry> {
        vec![
            entry(Some("/p1"), "/p1/w1", 1),
            entry(Some("/p1"), "/p1/w2", 2),
            entry(Some("/p2"), "/p2/w3", 3),
        ]
    }

    #[test]
    fn a_close_lands_on_the_successor() {
        assert_eq!(
            ring_landing(&spec_ring(), &[1], None),
            Some((ws("/p1/w2"), 2))
        );
    }

    #[test]
    fn a_close_at_the_tail_lands_on_the_predecessor() {
        assert_eq!(
            ring_landing(&spec_ring(), &[3], None),
            Some((ws("/p1/w2"), 2))
        );
    }

    /// A worktree deletion takes every session in the workspace at once, so
    /// the successor is measured past the last of them and the predecessor
    /// before the first.
    #[test]
    fn a_deletion_steps_over_every_session_it_removed() {
        let ring = vec![
            entry(Some("/p1"), "/p1/w1", 1),
            entry(Some("/p1"), "/p1/w2", 2),
            entry(Some("/p1"), "/p1/w2", 3),
            entry(Some("/p2"), "/p2/w3", 4),
        ];
        assert_eq!(ring_landing(&ring, &[2, 3], None), Some((ws("/p2/w3"), 4)));
    }

    #[test]
    fn an_empty_ring_and_an_unknown_removal_have_no_landing() {
        assert_eq!(ring_landing(&[], &[1], None), None);
        assert_eq!(ring_landing(&spec_ring(), &[99], None), None);
    }

    #[test]
    fn removing_everything_leaves_no_landing() {
        assert_eq!(ring_landing(&spec_ring(), &[1, 2, 3], None), None);
    }

    #[test]
    fn prefer_project_takes_its_own_project_over_a_nearer_neighbour() {
        // p2's session sits between the two p1 sessions, so a global search
        // from id 1 finds id 9 and a project-preferring one finds id 2.
        let ring = vec![
            entry(Some("/p1"), "/p1/w1", 1),
            entry(Some("/p2"), "/p2/w3", 9),
            entry(Some("/p1"), "/p1/w2", 2),
        ];
        assert_eq!(ring_landing(&ring, &[1], None), Some((ws("/p2/w3"), 9)));
        assert_eq!(
            ring_landing(&ring, &[1], Some(Path::new("/p1"))),
            Some((ws("/p1/w2"), 2))
        );
    }

    /// `ring_project` is `ring_global` plus a first pass, so the two must
    /// agree whenever that pass finds nothing.
    #[test]
    fn prefer_project_falls_through_to_the_whole_ring() {
        let ring = spec_ring();
        assert_eq!(
            ring_landing(&ring, &[3], Some(Path::new("/p2"))),
            ring_landing(&ring, &[3], None),
        );
    }

    #[test]
    fn home_has_no_project_to_prefer() {
        let ring = vec![entry(None, "/home-placeholder", 1), entry(Some("/p1"), "/p1/w1", 2)];
        assert_eq!(ring_landing(&ring, &[1], None), Some((ws("/p1/w1"), 2)));
    }

    /// A path two projects list is in the ring twice with one owner, so
    /// either occurrence resolves to the same landing.
    #[test]
    fn a_duplicated_workspace_changes_no_landing() {
        let ring = vec![
            entry(Some("/p1"), "/shared", 1),
            entry(Some("/p1"), "/shared", 1),
            entry(Some("/p2"), "/p2/w", 2),
        ];
        assert_eq!(ring_landing(&ring, &[1], None), Some((ws("/p2/w"), 2)));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```sh
cargo nextest run -p alacritree ring_landing a_close_lands a_deletion_steps prefer_project home_has_no_project a_duplicated_workspace
```

Expected: compilation failure, `cannot find type 'RingEntry'`.

- [ ] **Step 3: Write `RingEntry` and `ring_landing`**

Add to `alacritree/src/app.rs` beside `close_landing` and `close_fallback`, which are the other pure removal-policy functions.

```rust
/// One session's place in the flat ring: workspaces in sidebar order, each
/// workspace's sessions in spawn order.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RingEntry {
    /// The owning project's root, from `project_of`.  None for home.
    project: Option<PathBuf>,
    workspace: WorkspaceKey,
    id: SessionId,
}

/// The session a removal lands on under the `ring_*` policies.  `ring` is the
/// flat session ring captured before the removal and `removed` is what left
/// it: one session for a close, a worktree's whole list for a delete.
/// Successor first, the earliest survivor past the last removed entry, else
/// the latest survivor before the first.
///
/// `prefer` is the removed workspace's owning project under `ring_project`,
/// and None under `ring_global` and for home.  When set, the search runs over
/// that project's entries before running over the whole ring.
///
/// A path two projects both list appears in the ring twice.  Both entries
/// carry the same `project_of` tag and name the same session, so a duplicate
/// changes no answer; indices are taken by first occurrence, the way
/// `session_ring_target` takes them.
fn ring_landing(
    ring: &[RingEntry],
    removed: &[SessionId],
    prefer: Option<&Path>,
) -> Option<(WorkspaceKey, SessionId)> {
    let positions: Vec<usize> =
        removed.iter().filter_map(|id| ring.iter().position(|e| e.id == *id)).collect();
    let first = *positions.iter().min()?;
    let last = *positions.iter().max()?;

    let search = |group: Option<&Path>| {
        let in_group = |e: &RingEntry| match group {
            Some(root) => e.project.as_deref() == Some(root),
            None => true,
        };
        let survives = |e: &RingEntry| !removed.contains(&e.id);
        ring[last + 1..]
            .iter()
            .find(|e| in_group(e) && survives(e))
            .or_else(|| ring[..first].iter().rev().find(|e| in_group(e) && survives(e)))
            .map(|e| (e.workspace.clone(), e.id))
    };

    prefer.and_then(|root| search(Some(root))).or_else(|| search(None))
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```sh
cargo nextest run -p alacritree ring_landing a_close_lands a_deletion_steps prefer_project home_has_no_project a_duplicated_workspace an_empty_ring removing_everything
```

Expected: 9 passed.

- [ ] **Step 5: Build the tagged ring**

Split `workspace_order` so both callers share one walk, and add the ring builder. Replace `workspace_order` in `alacritree/src/app.rs` (currently 1621) with:

```rust
    /// Every workspace the app is willing to switch to, in sidebar order,
    /// each paired with its owning project's root.  Duplicates are kept:
    /// git lets two projects list one path, and dropping the second would
    /// change what `cycle_workspaces` visits for a user who configured
    /// nothing.
    fn workspace_order_with_projects(&self) -> Vec<(Option<PathBuf>, WorkspaceKey)> {
        let mut order: Vec<(Option<PathBuf>, WorkspaceKey)> = vec![(None, None)];
        for project in &self.projects {
            for wt in &project.worktrees {
                let has_sessions = self.workspace_has_sessions(&Some(wt.path.clone()));
                if worktree_is_switchable(wt, self.liveness.missing(&wt.path), has_sessions) {
                    let key = Some(wt.path.clone());
                    let owner = sidebar_nav::project_of(&self.projects, &key)
                        .map(Path::to_path_buf);
                    order.push((owner, key));
                }
            }
        }
        order
    }

    fn workspace_order(&self) -> Vec<WorkspaceKey> {
        self.workspace_order_with_projects().into_iter().map(|(_, ws)| ws).collect()
    }

    /// The flat session ring, tagged with each workspace's owning project.
    /// Callers build it only under a ring policy: it allocates per removal.
    fn session_ring(&self) -> Vec<RingEntry> {
        self.workspace_order_with_projects()
            .into_iter()
            .flat_map(|(project, workspace)| {
                self.workspace_session_indices(&workspace)
                    .into_iter()
                    .map(|i| RingEntry {
                        project: project.clone(),
                        workspace: workspace.clone(),
                        id: self.sessions[i].id,
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }
```

The owner is looked up through `project_of` rather than taken from the loop's `project`, so both occurrences of a duplicated path carry the same tag.

- [ ] **Step 6: Extend `CloseFallback` and `apply_close_fallback`**

Add the variant to the enum at `alacritree/src/app.rs:6255`:

```rust
    /// A session in another workspace, chosen by `ring_landing`.
    ActivateSession(SessionId),
```

Add the arm to `apply_close_fallback` (1385). Update its doc comment, which currently lists three outcomes:

```rust
    /// Act on a removal verdict: stay put, move to the project's main
    /// checkout, move to a session the ring chose, or go home.
    fn apply_close_fallback(&mut self, ctx: &Context, verdict: CloseFallback) {
        match verdict {
            CloseFallback::Stay => {},
            CloseFallback::Activate(main) => {
                self.activate_worktree(ctx, &main);
                // Adopting an existing idle session produces no PTY event, so
                // nothing else would wake the paint that shows it.
                ctx.request_repaint();
            },
            CloseFallback::ActivateSession(id) => {
                self.activate_session_by_id(id);
                ctx.request_repaint();
            },
            CloseFallback::Home => {
                self.activate_home(ctx);
                ctx.request_repaint();
            },
        }
    }
```

- [ ] **Step 7: Wire `close_session`**

In `close_session` (1337), capture the ring before the removal and consult it after the verdict. The capture goes immediately after the `workspace` binding and before `self.sessions.remove(idx)`:

```rust
        let workspace = self.sessions[idx].working_directory.clone();
        let policy = self.config.ui.last_session_close;
        let ring = policy.rings().then(|| self.session_ring()).unwrap_or_default();
        self.sessions.remove(idx);
```

Then replace the verdict block. The existing `respawn` early return and the deferred branch stay exactly as they are; only the verdict gains a ring override between them:

```rust
        let main = workspace.as_deref().and_then(|p| project_main_for(&self.projects, p));
        let mut verdict = close_fallback(&workspace, &self.current_workspace, &remaining, main);
        if verdict != CloseFallback::Stay && policy.rings() {
            let prefer = policy
                .prefers_project()
                .then(|| sidebar_nav::project_of(&self.projects, &workspace))
                .flatten();
            if let Some((_, landing)) = ring_landing(&ring, &[id], prefer) {
                verdict = CloseFallback::ActivateSession(landing);
            }
        }
```

`prefer` is resolved against `self.projects`, which the removal did not touch, so the deleted session's workspace is still listed. Leave the comment above `main` unchanged except to extend its sentence about `navigate` with the ring policies.

**Leave the reconciler alone.** Under `sidebar_focus = "follow"` the deferred path applies the cursor's own landing when that landing has a live session and the carried verdict only when it does not (`app.rs:2274-2280`). That precedence is deliberate and stays: `follow` means the landing row decides. It is not a bug to fix while passing through. The motivating case is unaffected either way, because an emptied worktree's cursor slides to the worktree row itself, which has no session, so the ring verdict applies.

- [ ] **Step 8: Verify**

```sh
cargo fmt
cargo nextest run -p alacritree
```

Expected: no failures. Fix any `dead_code` warning by checking that `session_ring` is genuinely reached from `close_session`.

- [ ] **Step 9: Commit**

```sh
git add alacritree/src/app.rs
git commit -m "feat(sidebar): land a close on the neighbouring session

Closing a workspace's last session left for the project main checkout or
home however many shells were running one row away.  Under the ring
policies the destination comes from the flat session ring instead, taken
before the removal because ring order is sidebar order and the removal
index is spawn order.

Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>"
```

---

### Task 4: worktree deletion honours the ring

Deleting the on-screen worktree hard-codes home. Under `follow` the cursor already slides to a sibling worktree row and the reconciler lands there, so the gap is `preserve`, and `follow` when the project has no other live worktree.

**Files:**
- Modify: `alacritree/src/app.rs:7590-7612` (`run_pending_delete`)
- Modify: `alacritree/src/app.rs:6325-6331` (`DeferredClose::removed_worktree` doc comment)

**Interfaces:**
- Consumes: `session_ring`, `ring_landing`, `CloseFallback::ActivateSession` (Task 3).
- Produces: nothing new.

- [ ] **Step 1: Capture the ring and the removed ids**

In `run_pending_delete`, the capture must precede the `retain` that drops the sessions. Replace the opening of the function through the `retain`:

```rust
    fn run_pending_delete(&mut self, ctx: &Context) {
        let Some(req) = self.pending_delete.take() else {
            return;
        };
        let project_root = self.projects[req.project_idx].root.clone();
        let policy = self.config.ui.last_session_close;
        let ring = policy.rings().then(|| self.session_ring()).unwrap_or_default();
        let removed: Vec<SessionId> = self
            .sessions
            .iter()
            .filter(|s| s.working_directory.as_deref() == Some(&req.worktree_path))
            .map(|s| s.id)
            .collect();

        // Drop sessions whose cwd is the worktree before deleting it; the PTY
        // would otherwise block the directory removal on some filesystems.
        self.sessions.retain(|s| s.working_directory.as_deref() != Some(&req.worktree_path));
        self.active_session.remove(&Some(req.worktree_path.clone()));
```

- [ ] **Step 2: Resolve the verdict and use it on both branches**

Replace the `if self.current_workspace.as_deref() == Some(&req.worktree_path)` block:

```rust
        if self.current_workspace.as_deref() == Some(&req.worktree_path) {
            let landing = policy
                .rings()
                .then(|| {
                    let prefer = policy
                        .prefers_project()
                        .then(|| {
                            sidebar_nav::project_of(
                                &self.projects,
                                &Some(req.worktree_path.clone()),
                            )
                        })
                        .flatten();
                    ring_landing(&ring, &removed, prefer)
                })
                .flatten();
            let verdict = match landing {
                Some((_, id)) => CloseFallback::ActivateSession(id),
                None => CloseFallback::Home,
            };
            if defers_close_navigation(self.config.ui.sidebar_focus) {
                self.sidebar_deferred_close = Some(DeferredClose {
                    verdict,
                    removed_worktree: Some(req.worktree_path.clone()),
                });
                ctx.request_repaint();
            } else {
                // Deleting the on-screen worktree is an explicit user action,
                // so the view should greet with a live shell rather than the
                // "no session" placeholder.
                self.apply_close_fallback(ctx, verdict);
            }
        }
```

`project_of` is called before the project refresh drops the worktree, so it still resolves.

- [ ] **Step 3: Correct the `removed_worktree` doc comment**

The field's comment at 6327 no longer describes the only pairing it sees. Replace it:

```rust
    /// Set when an asynchronous worktree deletion is in flight: `projects`
    /// still lists it, so without this the reconciler would see an intact row
    /// and could spawn a shell inside the directory being removed.  It pairs
    /// with any verdict, including a ring landing in another project.
    removed_worktree: Option<PathBuf>,
```

- [ ] **Step 4: Verify**

```sh
cargo fmt
cargo nextest run -p alacritree
```

Expected: no failures. `ring_landing`'s multi-id test from Task 3 is the unit cover for this path; the wiring itself needs a running app and is not unit-tested.

- [ ] **Step 5: Manual check**

```sh
cargo run -p alacritree
```

With `last_session_close = "ring_global"` and `sidebar_focus = "preserve"` in `alacritree.toml`: open two projects, put a session in each, delete the on-screen worktree, and confirm the view lands on the other project's session rather than on home. Then set `last_session_close = "respawn"` and confirm deletion still lands on home.

- [ ] **Step 6: Commit**

```sh
git add alacritree/src/app.rs
git commit -m "feat(sidebar): land a worktree deletion on the ring

Deleting the on-screen worktree hard-coded home, so a ring policy held
for a close and not for a delete.  The ring is captured before the
retain that drops the worktree's sessions, and all of them are removed
at once rather than one at a time.

Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>"
```

---

### Task 5: `sidebar_follow_active`

**Files:**
- Modify: `alacritree/src/config.rs` (`Ui` field, default, `RawUi` field, resolve, tests)
- Modify: `alacritree/src/sidebar_nav.rs` (add `follow_scroll_row` and tests)
- Modify: `alacritree/src/app.rs` (add `last_followed`, the panel wiring, the five scroll expressions)
- Modify: `schema/alacritree-config.json`, `docs/alacritree.md`

**Interfaces:**
- Consumes: `sidebar_nav::project_of` (Task 1).
- Produces: `pub fn follow_scroll_row(rows: &[SidebarRow], workspace: &WorkspaceKey, displayed: Option<SessionId>, project_root: Option<&Path>) -> Option<SidebarRow>`; `Ui::sidebar_follow_active: bool`.

`follow_scroll_row` is named to avoid colliding with `sidebar_focus::follow_target`, which resolves a cursor landing rather than a scroll target.

- [ ] **Step 1: Write the failing config tests**

In `alacritree/src/config.rs`'s test module:

```rust
    #[test]
    fn sidebar_follow_active_defaults_to_off() {
        assert!(!ui_from_toml("").sidebar_follow_active);
    }

    #[test]
    fn sidebar_follow_active_parses() {
        assert!(ui_from_toml("[ui]\nsidebar_follow_active = true").sidebar_follow_active);
    }
```

- [ ] **Step 2: Write the failing target-resolution tests**

In `alacritree/src/sidebar_nav.rs`'s test module. Reuse the `project_at` / `wt_at` helpers Task 1 added.

```rust
    #[test]
    fn the_displayed_session_row_is_the_target() {
        let rows = vec![
            SidebarRow::Home,
            SidebarRow::Project(PathBuf::from("/p1")),
            SidebarRow::Worktree(PathBuf::from("/p1/w1")),
            SidebarRow::Session(7),
        ];
        assert_eq!(
            follow_scroll_row(&rows, &Some(PathBuf::from("/p1/w1")), Some(7), Some(Path::new("/p1"))),
            Some(SidebarRow::Session(7))
        );
    }

    /// A workspace under the listing threshold, or one whose session a search
    /// filtered out, paints no session row.
    #[test]
    fn an_unpainted_session_falls_back_to_its_workspace_row() {
        let rows = vec![
            SidebarRow::Home,
            SidebarRow::Project(PathBuf::from("/p1")),
            SidebarRow::Worktree(PathBuf::from("/p1/w1")),
        ];
        assert_eq!(
            follow_scroll_row(&rows, &Some(PathBuf::from("/p1/w1")), Some(7), Some(Path::new("/p1"))),
            Some(SidebarRow::Worktree(PathBuf::from("/p1/w1")))
        );
    }

    #[test]
    fn a_collapsed_project_falls_back_to_its_header() {
        let rows = vec![SidebarRow::Home, SidebarRow::Project(PathBuf::from("/p1"))];
        assert_eq!(
            follow_scroll_row(&rows, &Some(PathBuf::from("/p1/w1")), Some(7), Some(Path::new("/p1"))),
            Some(SidebarRow::Project(PathBuf::from("/p1")))
        );
    }

    #[test]
    fn home_resolves_to_its_own_row() {
        let rows = vec![SidebarRow::Home];
        assert_eq!(follow_scroll_row(&rows, &None, None, None), Some(SidebarRow::Home));
    }

    /// A session whose project was removed renders on no row at all, so
    /// there is nothing to scroll to and the caller must retry later.
    #[test]
    fn a_detached_session_has_no_target() {
        let rows = vec![SidebarRow::Home];
        assert_eq!(
            follow_scroll_row(&rows, &Some(PathBuf::from("/gone")), Some(7), None),
            None
        );
    }
```

- [ ] **Step 3: Run both sets to verify they fail**

```sh
cargo nextest run -p alacritree sidebar_follow_active follow_scroll_row the_displayed_session an_unpainted_session a_collapsed_project home_resolves a_detached_session
```

Expected: compilation failure on both `sidebar_follow_active` and `follow_scroll_row`.

- [ ] **Step 4: Add the config key**

Four edits in `alacritree/src/config.rs`, each beside the corresponding `sidebar_focus` line.

`Ui` struct (near 892):

```rust
    /// Whether the projects sidebar scrolls to the session on screen when it
    /// changes.
    pub sidebar_follow_active: bool,
```

Defaults (near 969): `sidebar_follow_active: false,`

`RawUi` (near 1960):

```rust
    /// Whether the projects sidebar scrolls to the session on screen whenever
    /// it changes — a cycling key, a click, the palette, an IPC request.
    /// The sidebar cursor is left where it was: `false` (default).
    sidebar_follow_active: Option<bool>,
```

Resolve (near 2208): `sidebar_follow_active: self.ui.sidebar_follow_active.unwrap_or(false),`

- [ ] **Step 5: Write `follow_scroll_row`**

Add to `alacritree/src/sidebar_nav.rs`, after `project_of`:

```rust
/// The row the panel scrolls to when the session on screen changes: the
/// displayed session's own row, else the nearest ancestor `rows` renders.
/// None when none of them do — a session whose project was removed has no
/// row at all — which tells the caller to leave its comparison unwritten and
/// try again once the tree renders it.
pub fn follow_scroll_row(
    rows: &[SidebarRow],
    workspace: &WorkspaceKey,
    displayed: Option<SessionId>,
    project_root: Option<&Path>,
) -> Option<SidebarRow> {
    let candidates = [
        displayed.map(SidebarRow::Session),
        Some(match workspace {
            None => SidebarRow::Home,
            Some(path) => SidebarRow::Worktree(path.clone()),
        }),
        project_root.map(|r| SidebarRow::Project(r.to_path_buf())),
    ];
    candidates.into_iter().flatten().find(|row| rows.contains(row))
}
```

- [ ] **Step 6: Run both sets to verify they pass**

```sh
cargo nextest run -p alacritree sidebar_follow_active follow_scroll_row the_displayed_session an_unpainted_session a_collapsed_project home_resolves a_detached_session
```

Expected: 7 passed.

- [ ] **Step 7: Add the tracking field**

In `AlacritreeApp`'s struct definition, beside `sidebar_cursor_moved` (480):

```rust
    /// The workspace and session the projects panel last scrolled to, so a
    /// change is detected by comparison rather than by every writer of those
    /// two fields remembering to raise a flag.  Written only once a scroll
    /// actually fires, so a change whose row renders nowhere is retried.
    last_followed: (WorkspaceKey, Option<SessionId>),
```

Initialise it in the constructor beside `sidebar_cursor_moved: false` (near 845): `last_followed: (None, None),`

- [ ] **Step 8: Wire the panel**

In the projects panel, replace the `scrolls` closure at 3258:

```rust
        let cursor_moved = std::mem::take(&mut self.sidebar_cursor_moved);
        let scrolls = |is_cursor: bool| is_cursor && cursor_moved;

        // egui keeps one scroll target per frame and the last writer wins, so
        // the two reasons to scroll are resolved here rather than by paint
        // order.  An explicit cursor move outranks following the terminal.
        let active_now = self.active_session.get(&self.current_workspace).copied();
        let wants_follow = self.config.ui.sidebar_follow_active
            && !cursor_moved
            && (self.last_followed.0 != self.current_workspace
                || self.last_followed.1 != active_now);
        let follow_row = if wants_follow {
            // Bound first: `current_project_rows` takes `&mut self`, so its
            // borrow has to end before `projects` is read below.
            let rows = match &self.sidebar_rows_cache {
                Some(rows) => rows.clone(),
                None => self.current_project_rows(),
            };
            let project_root = sidebar_nav::project_of(&self.projects, &self.current_workspace)
                .map(Path::to_path_buf);
            sidebar_nav::follow_scroll_row(
                &rows,
                &self.current_workspace,
                active_now,
                project_root.as_deref(),
            )
        } else {
            None
        };
        if follow_row.is_some() {
            self.last_followed = (self.current_workspace.clone(), active_now);
        }
        let follows = |row: SidebarRow| follow_row.as_ref() == Some(&row);
```

- [ ] **Step 9: Extend the five scroll expressions**

Each of the five `scrolls(...)` call sites gains the follow reason. In order of appearance:

- 3498 (home row): `scrolls(home_is_cursor) || follows(SidebarRow::Home)`
- 3515 (home's session rows): `scrolls(is_cursor) || follows(SidebarRow::Session(row.id))`
- 3678 (project header): `if scrolls(header_is_cursor) || follows(SidebarRow::Project(project.root.clone()))`
- 3811 (worktree row): `scrolls(is_cursor) || follows(SidebarRow::Worktree(wt.path.clone()))`
- 3874 (a worktree's session rows): `scrolls(is_cursor) || follows(SidebarRow::Session(row.id))`

- [ ] **Step 10: Regenerate the schema and document the key**

```sh
ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema
cargo test -p alacritree --test config_schema
```

Add `sidebar_follow_active = false` with a one-line comment to the annotated `[ui]` block in `docs/alacritree.md`, beside `sidebar_focus`, and add it to the key list near 621.

- [ ] **Step 11: Verify**

```sh
cargo fmt
cargo nextest run -p alacritree
```

Expected: no failures, including `steady_state.rs`.

- [ ] **Step 12: Manual check**

```sh
cargo run -p alacritree
```

With `sidebar_follow_active = true`: open enough worktrees that the sidebar scrolls, cycle sessions with `SelectNextSession`, and confirm the panel follows. Press the sidebar-focus binding and confirm the cursor is still where you left it, not on the session you cycled to.

- [ ] **Step 13: Commit**

```sh
git add alacritree/src/config.rs alacritree/src/sidebar_nav.rs alacritree/src/app.rs schema/alacritree-config.json docs/alacritree.md
git commit -m "feat(sidebar): scroll to the session navigation lands on

Every session-switching path wrote the active session and stopped, so
the panel kept showing a workspace the terminal had left.  Detection is
a comparison rather than a flag, which no future writer of those fields
can forget to raise.

Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>"
```

---

### Task 6: `sidebar_scroll_align`

**Files:**
- Modify: `alacritree/src/config.rs` (enum, parser, `Ui` field, default, `RawUi` field, resolve, tests)
- Modify: `alacritree/src/app.rs` (`Theme` field, its construction, the five `scroll_to_rect` calls)
- Modify: `schema/alacritree-config.json`, `docs/alacritree.md`

**Interfaces:**
- Consumes: nothing.
- Produces: `pub enum ScrollAlign { Minimal, Center }` with `pub fn align(self) -> Option<egui::Align>`; `Ui::sidebar_scroll_align: ScrollAlign`.

`ScrollAlign::align` returns `Option<egui::Align>` because that is exactly what `scroll_to_rect` takes, and `None` is egui's own name for minimal scroll.

- [ ] **Step 1: Write the failing tests**

In `alacritree/src/config.rs`'s test module:

```rust
    #[test]
    fn sidebar_scroll_align_defaults_to_minimal() {
        assert_eq!(ui_from_toml("").sidebar_scroll_align, ScrollAlign::Minimal);
    }

    #[test]
    fn sidebar_scroll_align_parses_all_values() {
        for (raw, expected) in
            [("minimal", ScrollAlign::Minimal), ("center", ScrollAlign::Center)]
        {
            let ui = ui_from_toml(&format!("[ui]\nsidebar_scroll_align = \"{raw}\""));
            assert_eq!(ui.sidebar_scroll_align, expected, "value {raw:?}");
        }
    }

    #[test]
    fn sidebar_scroll_align_invalid_falls_back_to_minimal() {
        let ui = ui_from_toml("[ui]\nsidebar_scroll_align = \"middle-ish\"");
        assert_eq!(ui.sidebar_scroll_align, ScrollAlign::Minimal);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```sh
cargo nextest run -p alacritree sidebar_scroll_align
```

Expected: compilation failure, `cannot find type 'ScrollAlign'`.

- [ ] **Step 3: Add the enum and parser**

In `alacritree/src/config.rs`, after `SidebarFocus` and its parser:

```rust
/// `[ui] sidebar_scroll_align`: where a row a sidebar scrolled to is parked.
/// Governs both panels and both reasons to scroll, because it describes the
/// resting position rather than what chose the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScrollAlign {
    /// egui's minimal scroll: move just far enough to bring the row into
    /// view, which leaves it against whichever edge it entered from.
    #[default]
    Minimal,
    /// Park the row in the middle of the panel.  egui clamps to the scroll
    /// range, so a short list stays put instead of overscrolling.
    Center,
}

impl ScrollAlign {
    pub fn align(self) -> Option<egui::Align> {
        match self {
            Self::Minimal => None,
            Self::Center => Some(egui::Align::Center),
        }
    }
}

fn parse_scroll_align(raw: Option<&str>) -> ScrollAlign {
    match raw {
        None => ScrollAlign::default(),
        Some("minimal") => ScrollAlign::Minimal,
        Some("center") => ScrollAlign::Center,
        Some(other) => {
            log::warn!("unknown ui.sidebar_scroll_align value {other:?}, using \"minimal\"");
            ScrollAlign::default()
        },
    }
}
```

`config.rs:21` already reads `use egui::Color32;`, so `egui::Align` resolves with no import change.

- [ ] **Step 4: Add the config field**

`Ui` struct: `pub sidebar_scroll_align: ScrollAlign,` with a doc comment. Defaults: `sidebar_scroll_align: ScrollAlign::default(),`. `RawUi`:

```rust
    /// Where a row the sidebar scrolled to is parked:
    /// "minimal" (default) | "center".  Under "center" every cursor step
    /// re-centres the list, and clicking a row near the panel edge scrolls it
    /// out from under the pointer.
    #[schemars(extend("enum" = ["minimal", "center"]))]
    sidebar_scroll_align: Option<String>,
```

Resolve: `sidebar_scroll_align: parse_scroll_align(self.ui.sidebar_scroll_align.as_deref()),`

- [ ] **Step 5: Run the tests to verify they pass**

```sh
cargo nextest run -p alacritree sidebar_scroll_align
```

Expected: 3 passed.

- [ ] **Step 6: Thread it through `Theme`**

Add to the `Theme` struct at `alacritree/src/app.rs:73`, beside `icon_tooltips` (115):

```rust
    scroll_align: Option<egui::Align>,
```

Set it where `Theme` is built from config, beside `icon_tooltips: config.ui.icon_tooltips` (176):

```rust
            scroll_align: config.ui.sidebar_scroll_align.align(),
```

- [ ] **Step 7: Use it at the five scroll sites**

Replace `None` with `theme.scroll_align` in each. `paint_git_row_cursor` takes `theme: &Theme` (`app.rs:5812`) and the three row painters take it by value, so `theme.scroll_align` reads the same at all five.

- `app.rs:3679` — `ui.scroll_to_rect(rect, theme.scroll_align);`
- `app.rs:5820` — `ui.scroll_to_rect(rect, theme.scroll_align);`
- `app.rs:6075` — `ui.scroll_to_rect(full_rect, theme.scroll_align);`
- `app.rs:6820` — `ui.scroll_to_rect(full_rect, theme.scroll_align);`
- `app.rs:6947` — `ui.scroll_to_rect(full_rect, theme.scroll_align);`

Confirm none remain:

```sh
rg -n "scroll_to_rect\(.*, None\)" alacritree/src/
```

Expected: no matches.

- [ ] **Step 8: Regenerate the schema and document the key**

```sh
ALACRITREE_UPDATE_SCHEMA=1 cargo test -p alacritree --test config_schema
cargo test -p alacritree --test config_schema
```

Add `sidebar_scroll_align = "minimal"` to the annotated `[ui]` block in `docs/alacritree.md` and to the key list near 621. The comment says plainly that `center` re-centres on every step, so nobody reads it as a scrolloff.

- [ ] **Step 9: Verify**

```sh
cargo fmt
cargo nextest run -p alacritree
```

Expected: no failures.

- [ ] **Step 10: Manual check**

```sh
cargo run -p alacritree
```

With `sidebar_scroll_align = "center"`: focus the sidebar, hold a navigation key, and confirm the cursor stays mid-panel instead of riding the bottom edge. Scroll to the top of the list and confirm it does not overscroll past the first row. Repeat in the git panel.

- [ ] **Step 11: Commit**

```sh
git add alacritree/src/config.rs alacritree/src/app.rs schema/alacritree-config.json docs/alacritree.md
git commit -m "feat(sidebar): option to centre the scrolled-to row

Every sidebar scroll asked egui for the minimal one, which pins the
cursor to whichever edge it entered from.  One key governs both panels
and both reasons to scroll, since it describes where a row rests rather
than what chose it.

Co-Authored-By: Claude Opus 5 (1M Context) <noreply@anthropic.com>"
```

---

## Before opening the PR

- [ ] Confirm the branch still sits on a live commit of its base: `git -C <worktree> merge-base --is-ancestor <recorded base> origin/<base>`. When it fails, replay: `git rebase --onto origin/<base> <recorded base> <branch>`.
- [ ] `cargo nextest run -p alacritree` and `cargo fmt --check` both clean.
- [ ] `git diff origin/<base>...HEAD -- schema/` shows the three new keys and nothing else.
- [ ] Open with `devkit issue pr create --ready`, base `master`, title carrying the next `[n]` marker, `--arg closes="64 65"` (the worktree's own issue #63 supplies the first `Closes` line), and `--arg stacked_on=<base PR number>`.
