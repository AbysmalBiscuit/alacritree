//! The data models and decision functions behind `AlacritreeApp`, split out
//! so they can be read and tested without the render pass around them. Nothing
//! here names an egui type: an item that paints belongs in the parent module.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use alacritty_terminal::tty::Shell;

use serde_json::{Value, json};

use crate::bindings::{BindingAction, KeyBinding, NamedAction};
use crate::command_palette::{self};
use crate::config::{
    AttachMode, FontConfig, SearchDepth, SearchScope, SidebarFocus, UiFont, UiTheme,
};
use crate::multiplexer::{CreatedPane, Launch, PaneTarget};
use crate::panel_filter::PanelFilter;
use crate::path_style::PathStyle;
use crate::projects::{Project, Worktree};
use crate::session::{LiveState, SessionActivity, SessionId, SessionKind, TermSize};
use crate::sidebar_nav::{self, SidebarRow};
use crate::workspace::WorkspaceKey;
use crate::wsl::{self};
use crate::wsl_helper::{self, WslProbe};
use crate::{herdr, ipc, jobs, path_style, sidebar_focus};

/// Logical-pixel (normal, heading) sizes for UI text.  `[ui.font] size`
/// overrides the normal size directly (same pt→px conversion as
/// `FontConfig::egui_size`); the heading keeps its existing ratio to normal
/// text.  Unset, both fall back to the `[font]`-derived values unchanged.
pub(super) fn ui_text_px(font: &FontConfig, ui_font: &UiFont) -> (f32, f32) {
    match ui_font.size {
        Some(pt) => {
            let normal = pt * 96.0 / 72.0;
            let heading = normal * (FontConfig::UI_HEADING_RATIO / FontConfig::UI_NORMAL_RATIO);
            (normal, heading)
        },
        None => (font.ui_normal_px(), font.ui_heading_px()),
    }
}

/// Which pane owns keyboard input.  The terminal re-requests egui focus
/// every frame while it owns this; anything else holding focus (modals
/// aside) must win here first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PaneFocus {
    Terminal,
    ProjectsSidebar,
    GitSidebar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FocusDir {
    Left,
    Right,
}

/// Where a dispatched binding action came from.  A keyboard action consumed
/// a real key press, so FocusLeft/FocusRight may re-synthesize it into the
/// PTY when the inner TUI should handle it.  An IPC action has no key press
/// to forward — the caller is typically that inner program declaring it has
/// no window in the requested direction, and passthrough would bounce the
/// key straight back to it.  A palette action consumed a key press too, but
/// arrives with the panel still searching over a row the query may have
/// hidden — so actions that need a browsing cursor are refused at this origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ActionOrigin {
    Keyboard,
    Palette,
    Ipc,
}

/// What a FocusLeft/FocusRight press does, decided by [`focus_move`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FocusMove {
    /// The TUI inside the terminal can still move that way — forward the
    /// Ctrl+Arrow to the PTY instead of switching panels.
    Passthrough,
    Focus(PaneFocus),
    Nothing,
}

/// Panel-focus decision for FocusLeft/FocusRight.  Panels sit in a fixed
/// `ProjectsSidebar ↔ Terminal ↔ GitSidebar` row; movement toward a hidden
/// panel is dropped (focus never opens a panel).  From the terminal, a
/// keyboard-originated move is forwarded to a running split-managing TUI
/// (`tui_running`, see [`Session::nav_tui_running`]): the TUI walks its own
/// splits and hands focus back with `alacritree action Focus…` once it has
/// no window left in that direction — which is why IPC moves never pass
/// through (see [`ActionOrigin`]).
pub(super) fn focus_move(
    focus: PaneFocus,
    dir: FocusDir,
    left_open: bool,
    right_open: bool,
    origin: ActionOrigin,
    tui_running: bool,
) -> FocusMove {
    if origin != ActionOrigin::Ipc && focus == PaneFocus::Terminal && tui_running {
        return FocusMove::Passthrough;
    }
    let target = match (focus, dir) {
        (PaneFocus::Terminal, FocusDir::Left) => left_open.then_some(PaneFocus::ProjectsSidebar),
        (PaneFocus::Terminal, FocusDir::Right) => right_open.then_some(PaneFocus::GitSidebar),
        (PaneFocus::ProjectsSidebar, FocusDir::Right) => Some(PaneFocus::Terminal),
        (PaneFocus::GitSidebar, FocusDir::Left) => Some(PaneFocus::Terminal),
        _ => None,
    };
    match target {
        Some(t) => FocusMove::Focus(t),
        None => FocusMove::Nothing,
    }
}

/// What the binding pass knows about the frame when it decides whether a
/// matched action may consume a key press.  Each field names a scope some
/// action is gated on; outside it the action stands aside.
#[derive(Clone, Copy, Default)]
pub(super) struct BindingScope {
    pub(super) sidebar_focused: bool,
    pub(super) git_focused: bool,
    pub(super) scratchpad_focused: bool,
    /// The terminal owns focus and the session on screen has exited, so no
    /// child is left to read the keys its bindings would otherwise consume.
    pub(super) exited_session_focused: bool,
}

/// What the binding pass needs to know about the session on screen.
#[derive(Clone, Copy)]
pub(super) struct SessionFocus {
    pub(super) scratchpad: bool,
    pub(super) exited: bool,
}

/// The scope a key press is judged in.  Kept apart from the frame it is read
/// from so the mapping can be pinned on its own: `exited_session_focused` is
/// what lets a bare `Enter` be a chord at all, and the filter chain below
/// cannot tell a correct mapping from an inverted one.
pub(super) fn binding_scope(
    focus: PaneFocus,
    palette_open: bool,
    active: Option<SessionFocus>,
) -> BindingScope {
    // The palette is a modal that owns every key while it is up.
    let active = active.filter(|_| focus == PaneFocus::Terminal && !palette_open);
    BindingScope {
        sidebar_focused: focus == PaneFocus::ProjectsSidebar && !palette_open,
        git_focused: focus == PaneFocus::GitSidebar && !palette_open,
        scratchpad_focused: active.is_some_and(|s| s.scratchpad),
        exited_session_focused: active.is_some_and(|s| s.exited),
    }
}

/// Whether a matched binding's key press should reach `action`, given what
/// currently owns keyboard focus. Filter actions are scoped to the
/// sidebar that owns them so a bare letter like `d` doesn't fire a git-panel
/// filter while the projects sidebar (or the terminal) has focus, and vice
/// versa. `terminal_only` actions additionally step aside for the scratchpad
/// editor, which wants those same keys for native text editing.
pub(super) fn valid_for_focus(action: &BindingAction, scope: BindingScope) -> bool {
    let focus_ok = match action {
        BindingAction::Named(n) if n.is_exited_session_scoped() => scope.exited_session_focused,
        BindingAction::Named(n) if n.is_projects_filter_scoped() => scope.sidebar_focused,
        BindingAction::Named(n) if n.is_git_filter_scoped() => scope.git_focused,
        BindingAction::Named(n) if n.is_sidebar_scoped() => scope.sidebar_focused,
        _ => true,
    };
    let terminal_only = match action {
        BindingAction::Chars(_) => true,
        BindingAction::Named(n) => n.is_terminal_only(),
        BindingAction::Unsupported(_) => false,
    };
    focus_ok && !(scope.scratchpad_focused && terminal_only)
}

/// The actions one key press dispatches. Stacked user bindings can mix a
/// scoped action with a global one on a single trigger, so each is judged on
/// its own; an empty result leaves the press in the event queue, which is what
/// keeps a bare-key binding — `Enter`, `Delete`, a plain letter — out of the
/// PTY's way while its scope is inactive.
pub(super) fn dispatched_actions(
    matched: Vec<&BindingAction>,
    scope: BindingScope,
) -> Vec<&BindingAction> {
    matched
        .into_iter()
        .filter(|a| valid_for_focus(a, scope))
        // Search actions are owned by the sidebar nav pass; here their default
        // Enter/Esc/Shift+Esc must fall through to the PTY when the terminal
        // (or a non-searching panel) has focus.
        .filter(|a| !matches!(a, BindingAction::Named(n) if n.is_search_scoped()))
        // Palette cursor moves are owned by the palette modal, which suppresses
        // this pass entirely while it is up. Reaching here means it is closed,
        // so their keys belong to the sidebar or the PTY.
        .filter(|a| !matches!(a, BindingAction::Named(n) if n.is_palette_scoped()))
        .collect()
}

/// Whether a workspace survives the projects panel's toggle dimension.
pub(super) fn project_toggles_pass(
    apply: bool,
    toggle_sessions: bool,
    has_sessions: bool,
    toggle_attention: bool,
    needs_attention: bool,
) -> bool {
    if !apply {
        return true;
    }
    (!toggle_sessions || has_sessions) && (!toggle_attention || needs_attention)
}

/// Whether a workspace counts as occupied for the sessions toggle: it holds a
/// live session, or — when `counts_detached` is set — a listed herdr agent
/// nothing is attached to.  `session_workspaces` is the workspace of every
/// live session; a folded lone shell (absent from `listed` below the row
/// threshold, but still in `session_workspaces`) passes either way.
pub(super) fn sessions_filter_passes(
    session_workspaces: &[WorkspaceKey],
    listed: &sidebar_nav::ListedRows,
    key: &WorkspaceKey,
    counts_detached: bool,
) -> bool {
    session_workspaces.contains(key)
        || (counts_detached
            && listed.get(key).is_some_and(|entries| {
                entries.iter().any(|e| matches!(e, sidebar_nav::WorkspaceEntry::Agent(..)))
            }))
}

/// The toggle identities the projects panel accepts.  The PR identities exist
/// only when polling does, or every PR state would read as unknown and the
/// filters could only ever empty the panel.
pub(super) fn project_filter_toggles(pr_status: bool) -> &'static [char] {
    if pr_status { &['s', 'a', 'o', 'd', 'm', 'c'] } else { &['s', 'a'] }
}

/// The projects-panel toggle a named action flips, or `None` for an action that
/// is not one of its filters.  `PanelFilter::toggle` ignores an identity it does
/// not allow and dispatch falls through on an unmatched action, so nothing at
/// the call site can catch a wrong pairing — assert it here instead.
pub(super) fn project_filter_identity(action: NamedAction) -> Option<char> {
    match action {
        NamedAction::ToggleSessionsFilter => Some('s'),
        NamedAction::ToggleAttentionFilter => Some('a'),
        NamedAction::TogglePrOpenFilter => Some('o'),
        NamedAction::TogglePrDraftFilter => Some('d'),
        NamedAction::TogglePrMergedFilter => Some('m'),
        NamedAction::TogglePrClosedFilter => Some('c'),
        _ => None,
    }
}

/// Whether any toggle dimension narrows the projects panel this frame —
/// session presence, attention, or PR state. `project_self` falls back to
/// plain fuzzy matching only when this is false.
pub(super) fn any_project_toggle_active(
    toggle_sessions: bool,
    toggle_attention: bool,
    any_pr: bool,
) -> bool {
    toggle_sessions || toggle_attention || any_pr
}

/// Whether a worktree survives the projects panel's PR dimension. Inert
/// when no PR toggle is active, so a worktree passes regardless of what
/// `pr_matches` holds for it. Once a PR toggle is active, a worktree
/// missing from `pr_matches` is excluded — its PR lookup hasn't landed.
pub(super) fn worktree_pr_passes(
    any_pr: bool,
    pr_matches: &HashMap<PathBuf, bool>,
    path: &Path,
) -> bool {
    !any_pr || pr_matches.get(path).copied().unwrap_or(false)
}

/// Whether `current_project_rows` resolves session and herdr-agent names for
/// `child_matches` this frame.  `[ui] search_depth` at its "workspaces"
/// default answers false unconditionally, so no child name is ever computed
/// and a query costs what matching workspace names alone costs.
pub(super) fn search_reaches_children(depth: SearchDepth, query_is_empty: bool) -> bool {
    depth == SearchDepth::Sessions && !query_is_empty
}

/// Whether the projects panel is filtering on PR state this frame.  A toggle
/// the scope has stood down narrows nothing, so it must not pull the cache
/// generation into the reconciler or reach `gh` for a collapsed project.
pub(super) fn any_pr_toggle_active(filter: &PanelFilter, scope: SearchScope) -> bool {
    filter.toggles_apply(scope)
        && ['o', 'd', 'm', 'c'].into_iter().any(|key| filter.is_toggled(key))
}

/// The cache generation the reconciler observes.  Held at `0` unless a PR
/// filter is active, so a banked result only invalidates a row set that
/// actually depends on PR state.
pub(super) fn pr_generation_for(generation: u64, any_pr_toggle_active: bool) -> u64 {
    if any_pr_toggle_active { generation } else { 0 }
}

/// Whether this worktree's PR state is polled this frame.  Collapsed projects
/// normally cost no `gh` processes, but a PR filter has to see every row or it
/// would hide worktrees for want of a lookup it declined to start.
pub(super) fn should_poll_pr(pr_enabled: bool, expanded: bool, any_pr_toggle: bool) -> bool {
    pr_enabled && (expanded || any_pr_toggle)
}

/// Drag-and-drop payload for reordering the project list.  Carries the dragged
/// project's root rather than its index so a background refresh that shifts the
/// list mid-drag can't drop onto the wrong project.
#[derive(Clone)]
pub(super) struct DraggedProject(pub(super) PathBuf);

/// Drag-and-drop payload for reordering sessions.  Carries the id rather than
/// a position so a spawn, close or reorder mid-drag can't retarget the drop.
#[derive(Clone)]
pub(super) struct DraggedSession(pub(super) SessionId);

/// Where the user lands when a herdr attach fails after switching them.  The
/// job answers frames later, so a switch made in between is theirs and
/// outranks the restore: `previous` is handed back only while `current` is
/// still the workspace the attach moved them to.
pub(super) fn workspace_after_failed_attach(
    current: &WorkspaceKey,
    switched_to: &WorkspaceKey,
    previous: WorkspaceKey,
) -> WorkspaceKey {
    if current == switched_to { previous } else { current.clone() }
}

pub(super) fn workspace_label_for(projects: &[Project], ws: &WorkspaceKey) -> String {
    let Some(path) = ws else {
        return "Home".to_string();
    };
    for project in projects {
        for wt in &project.worktrees {
            if &wt.path == path {
                return format!("{} / {}", project.display_name(), wt.name);
            }
        }
    }
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| wsl::display_path(path))
}

/// What the session on screen contributes to a spawn's geometry.
pub(super) struct ActiveGeometry {
    pub(super) size: TermSize,
    pub(super) cell_size: (f32, f32),
    /// A scratchpad's size is fixed at construction: it takes the editor
    /// branch, so the pane never resizes it.
    pub(super) is_scratchpad: bool,
}

/// Geometry a new PTY is born at, most exact source first: the active
/// session's own numbers, then the terminal pane's last painted size, then
/// the constant neither has anything to improve on.
///
/// A scratchpad drops out of the first tier, since its pinned size would
/// otherwise shadow the pane geometry with a constant worse than the tier
/// below it.
pub(super) fn spawn_geometry(
    active: Option<ActiveGeometry>,
    last_pane: Option<(TermSize, (f32, f32))>,
) -> (TermSize, (f32, f32)) {
    active
        .filter(|active| !active.is_scratchpad)
        .map(|active| (active.size, active.cell_size))
        .or(last_pane)
        .unwrap_or((TermSize::new(80, 24), (8.0, 16.0)))
}

/// What one session contributes to the GUI's own priority boost for a frame.
pub(super) struct SessionBoost {
    /// The session's job holds a boost of its own.
    pub(super) raised: bool,
    /// The session is the one on screen, with the window focused.
    pub(super) visible: bool,
    /// The session's PTY is still opening.
    pub(super) pending: bool,
}

/// Whether this session is a reason for the GUI to stay boosted.  A session
/// still opening its PTY has no job to raise yet but will have one within a
/// frame or two, and counting it is what stops a spawn dropping the GUI to
/// normal priority for the whole open and raising it again on attach.
pub(super) fn holds_self_boost(session: SessionBoost) -> bool {
    session.raised || (session.visible && session.pending)
}

/// Whether a frame's sessions, taken together, are a reason for the GUI to
/// stay boosted.  Folded rather than `any`, because the caller computes each
/// `SessionBoost` by asking a session to raise or drop its own boost: every
/// session has to be reached, whatever the sessions before it answered.
pub(super) fn frame_holds_self_boost(boosts: impl Iterator<Item = SessionBoost>) -> bool {
    boosts.fold(false, |held, session| held | holds_self_boost(session))
}

pub(super) fn wsl_shell(distro: &str, workdir: &Path) -> Shell {
    let (program, args) = wsl::shell_invocation(distro, workdir);
    Shell::new(program, args)
}

/// Shimmed when the resident helper is on; the plain wsl.exe login-shell
/// launch (and an unknown probe) otherwise.
pub(super) fn wsl_session_shell(distro: &str, workdir: &Path) -> (Option<Shell>, Option<WslProbe>) {
    if !wsl_helper::enabled() {
        return (Some(wsl_shell(distro, workdir)), None);
    }
    let key = wsl_helper::new_probe_key();
    let (program, args) = wsl_helper::shim_invocation(distro, workdir, &key);
    (Some(Shell::new(program, args)), Some(WslProbe { distro: distro.to_string(), key }))
}

/// The probe shim for any user-supplied wsl.exe argv (profile or
/// `[terminal.shell]`): `Some` only when the argv is fully understood and
/// a distro name is known — the probe registry needs one, so a wrapped
/// default-distro launch resolves it via enumeration.  Anything exotic
/// runs unmodified and probes as unknown.
pub(super) fn shimmed_wsl_argv(program: &str, args: &[String]) -> Option<(Shell, WslProbe)> {
    if !wsl_helper::enabled() {
        return None;
    }
    let key = wsl_helper::new_probe_key();
    let (args, distro) = wsl_helper::wrap_profile_argv(program, args, &key)?;
    let distro =
        distro.or_else(|| wsl::distros().into_iter().find(|d| d.is_default).map(|d| d.name))?;
    Some((Shell::new(program.to_string(), args), WslProbe { distro, key }))
}

pub(super) fn profile_session_shell(
    profile: &crate::config::Profile,
) -> (Option<Shell>, Option<WslProbe>) {
    match shimmed_wsl_argv(&profile.program, &profile.args) {
        Some((shell, probe)) => (Some(shell), Some(probe)),
        None => (Some(profile_shell(profile)), None),
    }
}

/// `[terminal.shell] program = "wsl.exe"` gets the same shim as a wsl.exe
/// profile; any other config shell (or none) spawns unchanged through
/// `Session::pending_shell`'s own config-shell default.
pub(super) fn config_session_shell(
    config: &crate::config::Config,
) -> (Option<Shell>, Option<WslProbe>) {
    match &config.shell {
        Some(s) => match shimmed_wsl_argv(&s.program, &s.args) {
            Some((shell, probe)) => (Some(shell), Some(probe)),
            None => (None, None),
        },
        None => (None, None),
    }
}

pub(super) fn profile_shell(profile: &crate::config::Profile) -> Shell {
    Shell::new(profile.program.clone(), profile.args.clone())
}

/// The modal frame's horizontal inner margin.  Any width budgeted against the
/// window has to leave room for it, so it lives apart from the frame itself.
pub(super) fn modal_pad_x(scale: f32) -> f32 {
    (16.0 * scale).round()
}

/// The palette's footer, naming the keys actually bound to its cursor moves so
/// a rebind shows up here instead of the hint quietly going stale.
pub(super) fn palette_hint(bindings: &[KeyBinding]) -> String {
    let mut parts = vec!["↑↓ move".to_string()];
    for (action, label) in [
        (NamedAction::PaletteTop, "top"),
        (NamedAction::PaletteBottom, "bottom"),
        (NamedAction::PalettePageUp, "page up"),
        (NamedAction::PalettePageDown, "page down"),
    ] {
        if let Some(key) = command_palette::first_key(bindings, action) {
            parts.push(format!("{key} {label}"));
        }
    }
    parts.push("Enter run".into());
    parts.push("Esc close".into());
    parts.join(" · ")
}

/// What a palette column does with text too wide for it.  epaint overruns the
/// column rather than splitting a word unless told it may break anywhere, so
/// the choice follows the content: prose can rely on its spaces, a lone
/// identifier or key chord cannot.
#[derive(Clone, Copy)]
pub(super) enum ColumnWrap {
    /// One line, ellipsized at the column edge — the scannable default.
    Clip,
    /// Wrap at word boundaries, stopping after `max_rows`.
    Words { max_rows: usize },
    /// Wrap mid-token if that is the only way to stay inside the column,
    /// stopping after `max_rows`.
    Anywhere { max_rows: usize },
}

impl ColumnWrap {
    pub(super) fn limits(self) -> (usize, bool) {
        match self {
            Self::Clip => (1, true),
            Self::Words { max_rows } => (max_rows, false),
            Self::Anywhere { max_rows } => (max_rows, true),
        }
    }
}

/// The hover text for a row: the full text of whatever its columns had to cut,
/// and nothing at all when everything already reads in place.
pub(super) fn elided_hover(columns: &[(bool, &str)]) -> Option<String> {
    let full: Vec<&str> =
        columns.iter().filter(|(elided, _)| *elided).map(|(_, text)| *text).collect();
    (!full.is_empty()).then(|| full.join("\n"))
}

/// How wide the palette's content may be.  A window too narrow for the
/// comfortable width sizes the palette against the window instead, so the modal
/// keeps a margin either side rather than running past both edges.
pub(super) fn palette_content_width(scale: f32, screen_w: f32) -> f32 {
    let budget = screen_w * PALETTE_SCREEN_FRACTION - 2.0 * modal_pad_x(scale);
    budget.min(PALETTE_WIDTH * scale).max(0.0)
}

/// The palette's comfortable content width, and the share of a window it may
/// take instead when the window cannot hold that.
pub(super) const PALETTE_WIDTH: f32 = 760.0;
pub(super) const PALETTE_SCREEN_FRACTION: f32 = 0.8;

/// The action and keys columns' fixed widths, and the narrowest the description
/// still reads at beside them.
pub(super) const PALETTE_ACTION_W: f32 = 200.0;
pub(super) const PALETTE_KEYS_W: f32 = 180.0;
pub(super) const PALETTE_DESC_MIN: f32 = 160.0;

/// Clear space between a row's status mark and the description after it.
pub(super) const PALETTE_MARK_GAP: f32 = 6.0;

/// Footprint every leading row marker claims, whichever glyph it ends up
/// drawing. Markers vary wildly in intrinsic width (`·` vs `✳`), so sizing the
/// slot to the glyph would start each row's label at a different x.
pub(super) const ROW_STATUS_ICON_W: f32 = 10.0;

pub(super) const ATTENTION_HINT: &str = "needs attention";
pub(super) const CODEX_LOADER_FRAMES: [&str; 10] =
    ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Geometry for the palette's `description | action | keys` grid.  Every row and
/// the header lay out against the same widths, so the columns line up down the
/// list instead of each row packing its own way.  A grid with room for the fixed
/// widths gets them; a tighter one shrinks all three by the same factor and
/// wraps their text, rather than letting the last column run off the edge.
pub(super) struct PaletteColumns {
    pub(super) width: f32,
    pub(super) pad: f32,
    /// The leading status-mark gutter: the mark's own footprint plus the space
    /// after it.  Every row claims it, marked or not, so the descriptions line
    /// up whether or not a row has a mark to show.
    pub(super) mark: f32,
    pub(super) desc: f32,
    pub(super) action: f32,
    pub(super) keys: f32,
    pub(super) gap: f32,
    /// Set once the grid is tighter than its fixed widths, at which point every
    /// column wraps instead of ellipsizing.
    pub(super) narrow: bool,
}

impl PaletteColumns {
    pub(super) fn new(scale: f32, width: f32) -> Self {
        let pad = 10.0 * scale;
        let gap = 14.0 * scale;
        let mark = (ROW_STATUS_ICON_W + PALETTE_MARK_GAP) * scale;
        let content = (width - 2.0 * pad - mark - 2.0 * gap).max(0.0);
        let action = PALETTE_ACTION_W * scale;
        let keys = PALETTE_KEYS_W * scale;
        let comfortable = PALETTE_DESC_MIN * scale + action + keys;
        if content >= comfortable {
            let desc = content - action - keys;
            return Self { width, pad, mark, desc, action, keys, gap, narrow: false };
        }
        let shrink = content / comfortable;
        Self {
            width,
            pad,
            mark,
            desc: PALETTE_DESC_MIN * scale * shrink,
            action: action * shrink,
            keys: keys * shrink,
            gap,
            narrow: true,
        }
    }

    /// How the action and keys columns lay out.  Their text is one unbroken
    /// token, so a narrow grid has to split it mid-word; a comfortable one
    /// keeps every row one line tall and ellipsizes the overflow.
    pub(super) fn token_wrap(&self) -> ColumnWrap {
        if self.narrow { ColumnWrap::Anywhere { max_rows: usize::MAX } } else { ColumnWrap::Clip }
    }

    /// Where a row's status mark sits.  Clear of the selected row's accent
    /// bar, which is painted hard against the row's left edge.
    pub(super) fn mark_x(&self, left: f32) -> f32 {
        left + self.pad
    }

    pub(super) fn desc_x(&self, left: f32) -> f32 {
        self.mark_x(left) + self.mark
    }

    pub(super) fn action_x(&self, left: f32) -> f32 {
        self.desc_x(left) + self.desc + self.gap
    }

    pub(super) fn keys_x(&self, left: f32) -> f32 {
        self.action_x(left) + self.action + self.gap
    }
}

/// The Zed style's span decomposition: the filename text, then — when there
/// is a parent to show — the parent text with its separating space already
/// folded in. Position carries the emphasis: span 0 always paints with the
/// filename emphasis, span 1 (if present) with the parent emphasis. Shared by
/// `path_label`'s job builder and its fidelity test so a regression in the
/// split logic fails the test that exercises the real render path.
pub(super) fn zed_spans(parts: &path_style::Parts) -> Vec<String> {
    if parts.parent.is_empty() {
        vec![format!("{}{}", parts.root, parts.name)]
    } else {
        vec![parts.name.clone(), format!(" {}{}", parts.root, parts.parent)]
    }
}

pub(super) fn loader_glyph(frame: usize) -> &'static str {
    CODEX_LOADER_FRAMES[frame % CODEX_LOADER_FRAMES.len()]
}

/// What the status slot says on hover.  A named agent is named, so a
/// workspace running several can be told apart without opening any of them.
pub(super) fn agent_hint(live: LiveState, name: Option<&str>) -> String {
    let doing = match live {
        LiveState::Idle => "is running",
        LiveState::Working => "is working",
        LiveState::Blocked => "is waiting for you",
    };
    format!("{} {doing}", name.unwrap_or("agent"))
}

/// What a row knows about its own state, in the order the status slot ranks
/// it.  Grouped rather than passed loose because the three answer one
/// question between them, and the slot draws whichever ranks highest.
pub(super) struct RowStatus<'a> {
    pub(super) attention: bool,
    pub(super) activity: SessionActivity,
    pub(super) managed: Option<&'a Managed>,
}

/// A session's status mark, independent of where it paints — the sidebar's
/// fixed slot and the palette's row both ask `session_status_mark` for the
/// identical session, so the two can never disagree about its state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SessionMark {
    Attention,
    Harness(HarnessMark),
    Agent(LiveState),
}

/// Priority: attention dot > the harness's own state mark > the agent's live
/// state.  A harness outranks the live axis because it watches the pane from
/// outside and alacritree only reads its title, so where both have a reading
/// the harness's is the better one — and drawing it in the harness's
/// vocabulary is what keeps a pane looking the same listed and attached.
///
/// Returns the word the mark explains on hover alongside it.  `None` covers a
/// shell session, which has no state to mark.
pub(super) fn session_status_mark(status: &RowStatus<'_>) -> Option<(SessionMark, String)> {
    if status.attention {
        return Some((SessionMark::Attention, ATTENTION_HINT.to_owned()));
    }
    if let Some(managed) = status.managed
        && let Some(mark) = managed.mark
    {
        return Some((
            SessionMark::Harness(mark),
            format!("{} says {}", managed.harness, mark.label),
        ));
    }
    match status.activity {
        SessionActivity::Agent { name, live } => {
            Some((SessionMark::Agent(live), agent_hint(live, name)))
        },
        SessionActivity::Shell => None,
    }
}

/// Destination index for moving the item at `from` so it lands before display
/// slot `insert_before` (counted in the pre-move list), or `None` for a no-op.
/// Removing `from` before inserting shifts every later slot down by one — the
/// off-by-one this isolates so it can be tested without an app.
pub(super) fn move_target(len: usize, from: usize, insert_before: usize) -> Option<usize> {
    if from >= len {
        return None;
    }
    let mut to = insert_before.min(len);
    if to > from {
        to -= 1;
    }
    (to != from).then_some(to)
}

/// Position a session dropped before display slot `insert_before` should walk
/// to.  Inside its own workspace the session is removed before it is inserted,
/// so `move_target` compensates for the slots that shift down; coming from
/// another workspace it is inserted into a list it is not in yet, where the
/// display slot already is the position.
pub(super) fn drop_position(
    same_workspace: bool,
    len: usize,
    from: usize,
    insert_before: usize,
) -> Option<usize> {
    if same_workspace { move_target(len, from, insert_before) } else { Some(insert_before) }
}

/// The neighbour swaps that walk the element at `indices[j]` to slot
/// `position` of `indices`.
///
/// `indices` are the absolute positions one workspace occupies inside the
/// session vector, which are not contiguous: swapping only across them keeps
/// every other workspace's sessions at the index they were at.  Swapping is
/// also what avoids a `Clone` bound on `Session`, which owns a PTY.
pub(super) fn walk_swaps(indices: &[usize], j: usize, position: usize) -> Vec<(usize, usize)> {
    let mut swaps = Vec::new();
    if indices.is_empty() || j >= indices.len() {
        return swaps;
    }
    let position = position.min(indices.len() - 1);
    let mut j = j;
    while j > position {
        swaps.push((indices[j - 1], indices[j]));
        j -= 1;
    }
    while j < position {
        swaps.push((indices[j], indices[j + 1]));
        j += 1;
    }
    swaps
}

/// The session a reorder key acts on.
///
/// A cursored session wins, then the workspace the cursor is resting on lends
/// its active session, and otherwise the session on screen moves.  The middle
/// case is what makes a held key work across a workspace boundary: a session
/// arriving alone in a workspace paints no row of its own, so the cursor
/// climbs to that workspace's row, and the next press must still find it.
///
/// `CloseSession` has the same first-and-last shape; `DeleteSelected` reads
/// the cursor whatever has focus, which is the wrong convention here — a key
/// pressed at the terminal should move the terminal you are looking at.
pub(super) fn reorder_subject(
    sidebar_focused: bool,
    cursor: Option<&SidebarRow>,
    home_active: impl Fn() -> Option<SessionId>,
    worktree_active: impl Fn(&Path) -> Option<SessionId>,
    on_screen: impl Fn() -> Option<SessionId>,
) -> Option<SessionId> {
    if sidebar_focused {
        match cursor {
            Some(SidebarRow::Session(id)) => return Some(*id),
            Some(SidebarRow::Home) => {
                if let Some(id) = home_active() {
                    return Some(id);
                }
            },
            Some(SidebarRow::Worktree(path)) => {
                if let Some(id) = worktree_active(path) {
                    return Some(id);
                }
            },
            _ => {},
        }
    }
    on_screen()
}

/// Everything a sidebar session row needs, snapshotted before the panel
/// closure so rendering doesn't borrow `self.sessions`.
pub(super) struct SessionRowData {
    pub(super) id: SessionId,
    pub(super) name: RowName,
    pub(super) needs_attention: bool,
    pub(super) activity: SessionActivity,
    /// This workspace's remembered active session (accent icon).
    pub(super) is_active: bool,
    /// Active *and* the workspace is current — the session on screen
    /// (row background highlight).
    pub(super) is_displayed: bool,
    /// Set while this session is attached to a harness-managed pane, so an
    /// attached agent's row still says where it lives and how to leave.
    pub(super) managed: Option<Managed>,
}

/// One painted row under a workspace, in the order the sidebar draws them.
/// Attaching turns a herdr row into a session row in place, so the two travel
/// as one list rather than as two blocks that would reorder on attach.
pub(super) enum WorkspaceRowData {
    Session(SessionRowData),
    Herdr(HerdrRowData),
}

impl WorkspaceRowData {
    /// Whether any of `rows` is a session of alacritree's own.  The workspace
    /// row shows aggregate attention and activity only while none is: with a
    /// list on screen, repeating its summary above it reads as noise.
    pub(super) fn any_session(rows: &[Self]) -> bool {
        rows.iter().any(|row| matches!(row, Self::Session(_)))
    }
}

/// Everything a sidebar herdr-agent row needs, snapshotted before the panel
/// closure so rendering doesn't borrow `self.herdr.endpoints`.
pub(super) struct HerdrRowData {
    pub(super) side: herdr::Side,
    pub(super) terminal_id: String,
    pub(super) pane_id: String,
    pub(super) name: RowName,
    pub(super) managed: Managed,
}

/// The external supervisor a pane belongs to.  Named rather than flagged
/// because a second harness would otherwise add a parallel boolean to every
/// row, and because what a row must say — whose mark to paint, how to get
/// out, whether the attach is exclusive — varies by harness rather than by
/// row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Managed {
    /// What the tooltip calls it.
    pub(super) harness: &'static str,
    /// The harness's own detach chord, already rendered.  `None` when its
    /// config could not be read or binds detach to nothing, both of which are
    /// reasons to stay quiet rather than name a chord the user may not have.
    pub(super) detach: Option<String>,
    /// The attach shares the harness's whole view rather than one pane, so
    /// the row says so before a resize reveals it.
    pub(super) shared_view: bool,
    /// The agent kind the harness detected, spelled the way it invokes it.
    pub(super) kind: Option<String>,
    /// The pane's own title, when it says something the kind does not.
    pub(super) title: Option<String>,
    /// How the harness draws the state it reports.  `None` when it is no
    /// longer reporting one — a pane alacritree still holds open after its
    /// harness stopped listing it.
    pub(super) mark: Option<HarnessMark>,
}

impl HerdrRowData {
    pub(super) fn from_agent(
        agent: &herdr::Agent,
        side: &herdr::Side,
        settings: &herdr::Settings,
        attach: AttachMode,
    ) -> Self {
        let name = herdr_display_name(agent);
        Self {
            side: side.clone(),
            terminal_id: agent.terminal_id.clone(),
            pane_id: agent.pane_id.clone(),
            name,
            managed: Managed::herdr(side, settings, attach, Some(agent)),
        }
    }
}

/// A row's name in two parts, ranked by weight rather than punctuation: the
/// identity, and the category standing in front of it as context.  `context`
/// is absent when the identity is already the category, so a row never spells
/// one thing twice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RowName {
    pub(super) text: String,
    pub(super) context: Option<String>,
}

impl RowName {
    pub(super) fn plain(text: String) -> Self {
        Self { text, context: None }
    }

    /// What a text filter matches this row on.  Both parts, because the row
    /// shows both, and the context only when the row spells it out: a query
    /// naming a category an identity already carries must not match twice.
    pub(super) fn search_text(self) -> String {
        match self.context {
            Some(context) => format!("{} {}", self.text, context),
            None => self.text,
        }
    }
}

pub(super) struct PaletteSessionContent {
    pub(super) primary: String,
    pub(super) subtitle: String,
    pub(super) secondary: String,
    pub(super) title_for_hover: String,
}

pub(super) fn herdr_cwd(agent: &herdr::Agent) -> Option<&str> {
    agent
        .foreground_cwd
        .as_deref()
        .filter(|cwd| !cwd.trim().is_empty())
        .or_else(|| agent.cwd.as_deref().filter(|cwd| !cwd.trim().is_empty()))
}

/// The middle column's words, most general first: where the row comes from,
/// what runs in it, what that is doing.  The kind is spelled out whether or
/// not the title repeats it, so every row in one state reads identically.
pub(super) fn session_middle(
    lead: Option<&str>,
    kind: Option<&str>,
    status: Option<&str>,
    fallback: &str,
) -> String {
    let mut parts: Vec<String> = lead.map(str::to_owned).into_iter().collect();
    parts.extend(kind.map(str::to_lowercase));
    parts.extend(status.map(str::to_owned));
    if kind.is_none() && status.is_none() {
        parts.push(fallback.to_string());
    }
    parts.join(" · ")
}

/// The second line names the workspace, blanked only on an exact string
/// match — a title that merely reads like a directory into the workspace is
/// not matched against it.  Only a titleless row gives the first line up to
/// the workspace, and then the second has nothing left to say.
pub(super) fn native_palette_content(
    title: String,
    workspace: String,
    kind: Option<&str>,
    status: Option<&str>,
    fallback: &str,
) -> PaletteSessionContent {
    let primary = if title.trim().is_empty() { workspace.clone() } else { title };
    PaletteSessionContent {
        title_for_hover: primary.clone(),
        secondary: session_middle(None, kind, status, fallback),
        subtitle: if workspace == primary { String::new() } else { workspace },
        primary,
    }
}

pub(super) fn herdr_subtitle(glyph: &str, location: Option<&str>) -> String {
    location.map(|location| format!("{glyph} {location}")).unwrap_or_else(|| glyph.to_string())
}

pub(super) fn herdr_palette_content(
    title: Option<String>,
    agent: &herdr::Agent,
    workspace: Option<&str>,
    glyph: &str,
    cwd_style: PathStyle,
    cwd_home: Option<&str>,
) -> PaletteSessionContent {
    let cwd = herdr_cwd(agent);
    let abbreviated_cwd = cwd.map(|cwd| path_style::render(cwd, cwd_style, cwd_home));
    let (primary, title_for_hover) =
        if let Some(title) = title.filter(|title| !title.trim().is_empty()) {
            (title.clone(), title)
        } else {
            match workspace {
                Some(workspace) => (workspace.to_string(), workspace.to_string()),
                None => match (abbreviated_cwd.clone(), cwd) {
                    (Some(cwd), Some(full_cwd)) => (cwd, full_cwd.to_string()),
                    _ => ("Home".to_string(), "Home".to_string()),
                },
            }
        };
    // A workspace names the project a pane belongs to, which its path does
    // not, so it holds the second line whatever the title says.
    let location = match workspace {
        Some(workspace) => Some(workspace.to_string()),
        None => abbreviated_cwd.clone().filter(|_| cwd != Some(primary.as_str())),
    };
    let subtitle = match location.filter(|location| *location != primary) {
        Some(location) => herdr_subtitle(glyph, Some(&location)),
        None => glyph.to_string(),
    };
    PaletteSessionContent {
        primary,
        subtitle,
        secondary: session_middle(
            Some("herdr"),
            agent.kind.as_deref(),
            agent.status.map(|status| status.label()),
            "shell",
        ),
        title_for_hover,
    }
}

pub(super) fn palette_hover(
    title: &str,
    kind: Option<&str>,
    status: Option<&str>,
    side: &str,
    workspace: Option<&Path>,
    cwd: Option<&str>,
    pane_id: Option<&str>,
    terminal_id: Option<&str>,
    managed: Option<&Managed>,
    activation: &str,
) -> String {
    let mut lines = vec![format!("Title: {title}")];
    lines.extend(kind.map(|kind| format!("Kind: {kind}")));
    lines.extend(status.map(|status| format!("Status: {status}")));
    lines.push(format!("Side: {side}"));
    lines.extend(workspace.map(|workspace| format!("Workspace: {}", wsl::display_path(workspace))));
    lines.extend(cwd.map(|cwd| format!("Cwd: {cwd}")));
    lines.extend(pane_id.map(|pane_id| format!("Pane: {pane_id}")));
    lines.extend(terminal_id.map(|terminal_id| format!("Terminal: {terminal_id}")));
    lines.extend(managed.map(managed_tooltip));
    lines.push(format!("Activate: {activation}"));
    lines.join("\n")
}

pub(super) fn session_fallback_kind(kind: &SessionKind) -> &'static str {
    match kind {
        SessionKind::Shell => "shell",
        SessionKind::Diff { .. } => "diff",
        SessionKind::Scratchpad { .. } => "scratchpad",
    }
}

/// Whether a shell row can say what it is doing.  Only a plain shell has a
/// foreground job to read; a diff or scratchpad row has no process behind it
/// and would be inventing a state.
pub(super) fn shell_state_for(kind: &SessionKind, busy: bool) -> Option<&'static str> {
    match kind {
        SessionKind::Shell => Some(if busy { "busy" } else { "idle" }),
        SessionKind::Diff { .. } | SessionKind::Scratchpad { .. } => None,
    }
}

/// The name herdr reports for a pane, and `None` when it reports none.  The
/// kind rides along as context unless it says the same thing as the title.
/// What a titleless agent falls back to differs by row, so each caller says
/// so itself rather than passing its answer through here.
pub(super) fn herdr_row_name(agent: &herdr::Agent) -> Option<RowName> {
    let title = agent.title.clone()?;
    let context = agent.kind.clone().filter(|kind| *kind != title);
    Some(RowName { text: title, context })
}

/// The sidebar row, the palette row and the text filter must all resolve an
/// agent's name the same way, or the filter stops matching what the other two
/// paint.  Falls back from herdr's title, to the agent's kind, to the last
/// six characters of its terminal id — a listed row has nothing better than
/// the terminal id's tail behind the kind, so the kind takes the name rather
/// than standing in front of six characters nobody reads.
pub(super) fn herdr_display_name(agent: &herdr::Agent) -> RowName {
    herdr_row_name(agent).unwrap_or_else(|| {
        RowName::plain(agent.kind.clone().unwrap_or_else(|| {
            let id = &agent.terminal_id;
            let skip = id.chars().count().saturating_sub(6);
            id.chars().skip(skip).collect()
        }))
    })
}

impl Managed {
    /// `agent` is herdr's current word on the pane, and `None` once it stops
    /// reporting one — a pane alacritree still holds open after its harness
    /// let go of it, which has a harness and a way out but no state or name.
    pub(super) fn herdr(
        side: &herdr::Side,
        settings: &herdr::Settings,
        attach: AttachMode,
        agent: Option<&herdr::Agent>,
    ) -> Self {
        let kind = agent.and_then(|a| a.kind.clone());
        let title =
            agent.and_then(|a| a.title.clone()).filter(|t| Some(t.as_str()) != kind.as_deref());
        Self {
            harness: "herdr",
            detach: settings.detach.clone(),
            shared_view: !herdr::attaches_directly(
                side,
                attach,
                agent.is_none_or(|a| a.status.is_some()),
            ),
            mark: agent
                .and_then(|a| a.status)
                .map(|status| herdr_mark(status, settings.indicators)),
            kind,
            title,
        }
    }

    /// What the harness calls this pane: the agent kind backquoted as the
    /// command it is, and the title quoted as the words it is.
    pub(super) fn pane_name(&self) -> Option<String> {
        match (&self.kind, &self.title) {
            (Some(kind), Some(title)) => Some(format!("`{kind}` \"{title}\"")),
            (Some(kind), None) => Some(format!("`{kind}`")),
            (None, Some(title)) => Some(format!("\"{title}\"")),
            (None, None) => None,
        }
    }
}

/// The mark a harness paints for the state it reports, in that harness's own
/// vocabulary.  Resolved once per row, so a pane reads the same whether it is
/// listed or attached — the two are drawn by different painters, and attaching
/// must not repaint a pane in a language it does not speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct HarnessMark {
    pub(super) glyph: &'static str,
    pub(super) tone: StateTone,
    /// The harness's own word for this state, for the hover text.
    pub(super) label: &'static str,
}

/// What a harness means by a state's color.  Named rather than carried as a
/// `Color32` so the palette stays alacritree's and a row snapshot stays free
/// of the theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StateTone {
    Blocked,
    Working,
    Done,
    Idle,
    /// The harness reported something alacritree does not recognise.
    Unclear,
}

/// herdr's state vocabulary, taken from its own `state_icon_symbol` and
/// `state_label_color`.  Which of the two sets applies is herdr's `[ui]
/// status_indicators`, so a user who picked one in herdr gets it here too.
///
/// `done` is `idle` on herdr's internal axis and a status of its own over its
/// API, which is the axis alacritree reads — so the two arrive already
/// distinguished, without the "has a human looked at it yet" bit herdr tracks
/// to tell them apart.
pub(super) fn herdr_mark(status: herdr::Status, indicators: herdr::Indicators) -> HarnessMark {
    use herdr::Indicators::{Dots, Symbols};
    use herdr::Status::{Blocked, Done, Idle, Unknown, Working};
    let (glyph, tone) = match (indicators, status) {
        (_, Idle) => ("○", StateTone::Idle),
        (_, Unknown) => ("·", StateTone::Unclear),
        (Dots, Blocked) => ("●", StateTone::Blocked),
        (Dots, Working) => ("●", StateTone::Working),
        (Dots, Done) => ("●", StateTone::Done),
        (Symbols, Blocked) => ("×", StateTone::Blocked),
        (Symbols, Working) => ("◐", StateTone::Working),
        (Symbols, Done) => ("✓", StateTone::Done),
    };
    HarnessMark { glyph, tone, label: status.label() }
}

/// Spawn-ordered ids of the sessions in `ws`, or empty below the list
/// threshold.  The threshold is normally two — a single-session workspace row
/// keeps its compact form, mirroring the tab strip — and `always` lowers it
/// to one.
///
/// herdr rows are never held back by it: a pane alacritree does not own has
/// no other surface to appear on, and hiding it would hide the workspace's
/// only row.  They do count toward the threshold, so a lone shell session
/// beside one is listed rather than folded into the workspace row, which
/// would leave a hole in a list its neighbours are already in.
///
/// `managed` carries herdr's own position for each of its rows, and sorts by
/// it here so an attached session and a listed agent interleave the way herdr
/// has them rather than by which kind of row they are.  The sort is stable,
/// so panes herdr no longer lists keep the order they were spawned in.
pub(super) fn workspace_entries(
    shells: &[SessionId],
    managed: Vec<(usize, sidebar_nav::WorkspaceEntry)>,
    always: bool,
) -> Vec<sidebar_nav::WorkspaceEntry> {
    let threshold = if always { 1 } else { 2 };
    let mut managed = managed;
    managed.sort_by_key(|(at, _)| *at);
    let mut entries = Vec::with_capacity(shells.len() + managed.len());
    if shells.len() + managed.len() >= threshold {
        entries.extend(shells.iter().copied().map(sidebar_nav::WorkspaceEntry::Session));
    }
    entries.extend(managed.into_iter().map(|(_, entry)| entry));
    entries
}

/// Step the lockstep index over the rows a skipped worktree owns.
///
/// The projection is built before the deletion is known, so it still lists
/// the worktree with everything under it.  Leaving the index parked on a row
/// no node will match again would mark every later node unprojected, and the
/// cursor repair reads an unprojected row as one that has gone away.
pub(super) fn skip_projected_rows(
    rows: &[SidebarRow],
    next_row: &mut usize,
    listed: &sidebar_nav::ListedRows,
    path: &Path,
) {
    if rows.get(*next_row) != Some(&SidebarRow::Worktree(path.to_path_buf())) {
        return;
    }
    *next_row += 1;
    for entry in listed.get(&Some(path.to_path_buf())).map_or(&[][..], Vec::as_slice) {
        if rows.get(*next_row) != Some(&entry.row()) {
            break;
        }
        *next_row += 1;
    }
}

/// Assemble the model arena and the projection.  `rows` is the projection —
/// exactly what the cursor steps over — and `live` is the model: every running
/// session, whatever the listing threshold or the filter says.  Building
/// membership from `listed` instead would make the last session in a workspace
/// read as deleted the moment its sibling closed.
///
/// `listed` is the listing the projection was built from.  A herdr row exists
/// only while its agent is listed, so there is no wider model to take it from,
/// and reading a second listing here could disagree with `rows`.
///
/// `skip_worktree` drops a worktree whose deletion is already committed but
/// whose git operation has not finished, so nothing lands the cursor — or a
/// new shell — inside a directory on its way out.
///
/// Nodes are pushed in exactly the order `sidebar_nav::visible_rows` emits,
/// with unprojected nodes interleaved, so one forward index into `rows`
/// classifies every node.  Asking `rows.contains` per node instead would be
/// quadratic in path comparisons on a path that runs whenever the user types.
pub(super) fn build_sidebar_snapshot(
    projects: &[Project],
    live: &[(WorkspaceKey, SessionId)],
    listed: &sidebar_nav::ListedRows,
    rows: &[SidebarRow],
    skip_worktree: Option<&Path>,
    inputs: sidebar_focus::ObservedInputs,
) -> sidebar_focus::TreeSnapshot {
    use sidebar_focus::Parent;
    use sidebar_nav::WorkspaceEntry;

    let mut b = sidebar_focus::SnapshotBuilder::default();
    let mut next_row = 0usize;
    let mut placed = vec![false; live.len()];

    // Consume `rows` in lockstep: a node is projected exactly when it is the
    // row the projection expects next.
    let push = |b: &mut sidebar_focus::SnapshotBuilder,
                next_row: &mut usize,
                row: SidebarRow,
                parent: Parent| {
        let projected = rows.get(*next_row) == Some(&row);
        if projected {
            *next_row += 1;
        }
        b.push(row, parent, projected)
    };
    let push_workspace = |b: &mut sidebar_focus::SnapshotBuilder,
                          next_row: &mut usize,
                          placed: &mut [bool],
                          ws: &WorkspaceKey,
                          parent: Parent| {
        let entries = listed.get(ws).map_or(&[][..], Vec::as_slice);
        for entry in entries {
            push(b, next_row, entry.row(), parent);
        }
        // A workspace lists every shell session it has or none of them, and a
        // session attached to a herdr pane is always listed, so a session
        // reaching the second arm here belongs to a workspace that listed
        // nothing at all.  It is running, so the model keeps it; it is drawn
        // nowhere, so the projection does not.
        for (i, (w, id)) in live.iter().enumerate() {
            if w != ws {
                continue;
            }
            placed[i] = true;
            if !entries.contains(&WorkspaceEntry::Session(*id)) {
                b.push(SidebarRow::Session(*id), parent, false);
            }
        }
    };

    let home_id = push(&mut b, &mut next_row, SidebarRow::Home, Parent::Root);
    push_workspace(&mut b, &mut next_row, &mut placed, &None, Parent::Node(home_id));

    for p in projects {
        let project_id =
            push(&mut b, &mut next_row, SidebarRow::Project(p.root.clone()), Parent::Root);
        for wt in &p.worktrees {
            if skip_worktree == Some(wt.path.as_path()) {
                skip_projected_rows(rows, &mut next_row, listed, &wt.path);
                continue;
            }
            let wt_id = push(
                &mut b,
                &mut next_row,
                SidebarRow::Worktree(wt.path.clone()),
                Parent::Node(project_id),
            );
            let ws = Some(wt.path.clone());
            push_workspace(&mut b, &mut next_row, &mut placed, &ws, Parent::Node(wt_id));
        }
    }

    // Sessions whose workspace has no row left — a removed project, or a
    // worktree already treated as gone.  They are running, so they belong in
    // the model; they have no place in the tree, so they are nobody's sibling.
    for (i, (_, id)) in live.iter().enumerate() {
        if !placed[i] {
            b.push(SidebarRow::Session(*id), Parent::Detached, false);
        }
    }

    debug_assert_eq!(next_row, rows.len(), "every projected row must be in the arena");
    b.finish(inputs)
}

/// The cursor, workspace, and active session the reconciler last wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SidebarFocusWrite {
    pub(super) cursor: Option<SidebarRow>,
    pub(super) workspace: WorkspaceKey,
    pub(super) active: Option<SessionId>,
}

/// Whether focus moved behind the reconciler's back.  The active session is
/// part of the comparison because the tab and session cycling actions can
/// switch sessions without leaving the workspace, changing nothing else.
/// Comparing the resulting state rather than matching on action names covers
/// every route to them — rebound keys, the command palette, MCP — at the price
/// of `ensure_active_session` and `adopt_active_session` marking their own
/// writes so their self-healing does not read as navigation.
pub(super) fn sidebar_focus_overtaken(
    written: &Option<SidebarFocusWrite>,
    cursor: Option<&SidebarRow>,
    workspace: &WorkspaceKey,
    active: Option<SessionId>,
) -> bool {
    match written {
        None => false,
        Some(w) => w.cursor.as_ref() != cursor || w.workspace != *workspace || w.active != active,
    }
}

/// Where the view goes after a session's removal.
#[derive(Debug, PartialEq)]
pub(super) enum CloseFallback {
    /// Removal didn't empty the on-screen workspace — no navigation.
    Stay,
    /// Switch to the project's main checkout, which still has a session.
    Activate(PathBuf),
    /// A session in another workspace, chosen by `ring_landing`.
    ActivateSession(SessionId),
    /// Switch to home; `activate_home` spawns a shell there if none exists.
    Home,
}

/// Why a session record is going away.  The distinction exists because
/// neither half of a close, the respawn policy or the navigation, may apply
/// to a session that never got a PTY.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum CloseReason {
    User,
    SpawnFailed,
}

/// The verdict a close acts on.  A failed open stays put whatever the
/// workspace's state says: every destination `close_fallback` can name is one
/// `ensure_active_session` will spawn into, and that open fails the same way.
/// Staying leaves the pane on the "no session" placeholder, which is what the
/// workspace honestly holds.
pub(super) fn close_navigation(reason: CloseReason, verdict: CloseFallback) -> CloseFallback {
    match reason {
        CloseReason::User => verdict,
        CloseReason::SpawnFailed => CloseFallback::Stay,
    }
}

/// Which session a workspace switches to when the one at `removed_idx` is
/// closed.  `sessions` is the list *after* removal; `removed_idx` indexes the
/// list *before* it, so the first surviving sibling at or past it is the
/// closed session's successor.  Pure over (workspace, id) pairs for the same
/// reason as `close_fallback`.
///
/// `Preserve` hands the workspace its first session whichever one closed.
/// `Follow` takes the successor, or the predecessor when the last session
/// closed — the ordinal rule `sidebar_focus::slide` lands the cursor by, so a
/// close that moves both cannot point them at different siblings.
pub(super) fn close_landing(
    sessions: &[(WorkspaceKey, SessionId)],
    workspace: &WorkspaceKey,
    removed_idx: usize,
    mode: SidebarFocus,
) -> Option<SessionId> {
    let mut siblings = sessions
        .iter()
        .enumerate()
        .filter(|(_, (w, _))| w == workspace)
        .map(|(i, (_, id))| (i, *id));
    if !mode.follows() {
        return siblings.next().map(|(_, id)| id);
    }
    let mut predecessor = None;
    for (i, id) in siblings {
        if i >= removed_idx {
            return Some(id);
        }
        predecessor = Some(id);
    }
    predecessor
}

/// Post-close navigation for the workspace that just lost a session.
/// `remaining` is the session list after removal; `main_checkout` is the
/// removed workspace's project main (None when the workspace *is* the main,
/// is home, or belongs to no known project). Pure over (workspace, id)
/// pairs for the same reason the sidebar listing does: the rule stays
/// testable without spawning PTYs.
pub(super) fn close_fallback(
    removed_ws: &WorkspaceKey,
    current_ws: &WorkspaceKey,
    remaining: &[(WorkspaceKey, SessionId)],
    main_checkout: Option<PathBuf>,
) -> CloseFallback {
    if removed_ws != current_ws || remaining.iter().any(|(w, _)| w == removed_ws) {
        return CloseFallback::Stay;
    }
    match main_checkout {
        Some(main) if remaining.iter().any(|(w, _)| w.as_deref() == Some(main.as_path())) => {
            CloseFallback::Activate(main)
        },
        _ => CloseFallback::Home,
    }
}

/// One session's place in the flat ring: workspaces in sidebar order, each
/// workspace's sessions in spawn order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RingEntry {
    /// The owning project's root, from `project_of`.  None for home.
    pub(super) project: Option<PathBuf>,
    pub(super) workspace: WorkspaceKey,
    pub(super) id: SessionId,
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
pub(super) fn ring_landing(
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

/// A close-fallback verdict the reconciler owes the terminal, and the worktree
/// whose rows must already read as gone.  The verdict is carried rather than
/// recomputed because only `close_fallback` knows the difference between
/// staying put, hopping to the project's main checkout, and going home.
#[derive(Debug)]
pub(super) struct DeferredClose {
    pub(super) verdict: CloseFallback,
    /// Set when an asynchronous worktree deletion is in flight: `projects`
    /// still lists it, so without this the reconciler would see an intact row
    /// and could spawn a shell inside the directory being removed.  It pairs
    /// with any verdict, including a ring landing in another project.
    pub(super) removed_worktree: Option<PathBuf>,
}

/// Whether the reconciler owns post-removal navigation.  Under `"follow"` the
/// landing row decides where the terminal goes, so acting here first would
/// show one workspace for a frame and another the next.
pub(super) fn defers_close_navigation(mode: SidebarFocus) -> bool {
    mode.follows()
}

/// What re-homing a session does to the active-session maps and the view.
/// Pure over the same kind of snapshot `close_fallback` takes, so the policy
/// is testable without spawning PTYs.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum SourceRepair {
    Keep,
    Set(SessionId),
    Remove,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct MoveOutcome {
    pub(super) source: SourceRepair,
    /// The moved session becomes the target workspace's active session.
    pub(super) claim_target: bool,
    /// Switch the view to the target — the user was watching this session.
    pub(super) follow: bool,
}

pub(super) fn plan_move(
    was_source_active: bool,
    on_screen: bool,
    next_in_source: Option<SessionId>,
    target_has_active: bool,
) -> MoveOutcome {
    let source = match (was_source_active, next_in_source) {
        (false, _) => SourceRepair::Keep,
        (true, Some(id)) => SourceRepair::Set(id),
        (true, None) => SourceRepair::Remove,
    };
    MoveOutcome { source, claim_target: on_screen || !target_has_active, follow: on_screen }
}

/// The owning project's main checkout for `ws`, or None when `ws` already
/// is the main (including non-git roots, whose single pseudo-worktree is
/// its own main) or belongs to no known project.
pub(super) fn project_main_for(projects: &[Project], ws: &Path) -> Option<PathBuf> {
    let root = sidebar_nav::project_of(projects, &Some(ws.to_path_buf()))?;
    let project = projects.iter().find(|p| p.root == root)?;
    let main = project.worktrees.iter().find(|w| w.is_main)?;
    if main.path == ws { None } else { Some(main.path.clone()) }
}

/// The project to expand so `row` still renders once search exits, if any.
/// Only child rows qualify: search lists matched worktrees and sessions whatever
/// their project's `expanded` flag says, so they vanish when the query clears.
/// A header is already its own row, and expanding it would turn selecting a
/// project into a toggle.
pub(super) fn search_reveal_root(
    projects: &[Project],
    session_workspace: impl Fn(SessionId) -> Option<WorkspaceKey>,
    row: &SidebarRow,
) -> Option<PathBuf> {
    if !matches!(row, SidebarRow::Worktree(_) | SidebarRow::Session(_)) {
        return None;
    }
    row_project_root(projects, session_workspace, row)
}

/// The root of the project owning `row`: a worktree resolves by its path, a
/// session through its workspace.  `None` for Home or a row outside every
/// known project.  Lets `ToggleProjectExpanded` act on the whole subtree, not
/// just the header.
pub(super) fn row_project_root(
    projects: &[Project],
    session_workspace: impl Fn(SessionId) -> Option<WorkspaceKey>,
    row: &SidebarRow,
) -> Option<PathBuf> {
    let workspace = match row {
        SidebarRow::Project(root) => return Some(root.clone()),
        SidebarRow::Worktree(path) => path.clone(),
        SidebarRow::Session(id) => session_workspace(*id).flatten()?,
        SidebarRow::Home => return None,
        // Carries a (Side, terminal id) pair, not a workspace or a SessionId,
        // so unlike a session row there is nothing here to resolve against.
        SidebarRow::HerdrAgent(..) => return None,
    };
    projects
        .iter()
        .find(|p| p.worktrees.iter().any(|w| w.path == workspace))
        .map(|p| p.root.clone())
}

/// The session a SelectNextSession/SelectPreviousSession press lands on:
/// one flat ring over every open session, workspaces in sidebar order and
/// each workspace's sessions in the order its rows are drawn.  `None` means stay put — a
/// ring too small to cycle, or an active session missing from the ring
/// (its worktree turned prunable).  With no active session (an emptied
/// workspace on screen) the first entry re-anchors the cycle.
pub(super) fn session_ring_target(
    ring: &[(WorkspaceKey, SessionId)],
    current: Option<SessionId>,
    delta: i32,
) -> Option<(WorkspaceKey, SessionId)> {
    if ring.len() < 2 {
        return None;
    }
    let Some(current) = current else {
        return Some(ring[0].clone());
    };
    let pos = ring.iter().position(|(_, id)| *id == current)?;
    let next = (pos as i32 + delta).rem_euclid(ring.len() as i32) as usize;
    Some(ring[next].clone())
}

/// The activity a session's row paints.  A session attached to a herdr agent
/// takes herdr's word, because herdr watches the pane from outside and sees an
/// approval dialog no title heuristic can reach.
///
/// `unknown` is herdr declining to say, so the session's own reading stands.
/// The gate closes either way: an attached pane holds an agent whether or not
/// the process probe recognized one.
pub(super) fn herdr_backed_activity(
    own: SessionActivity,
    status: Option<herdr::Status>,
) -> SessionActivity {
    let Some(status) = status else { return own };
    let live = LiveState::from_herdr(status).unwrap_or(own.live().unwrap_or_default());
    own.with_live(live)
}

/// Agent titles commonly lead with their own decorative mark. Once the row
/// paints a semantic agent/loader status, retaining that mark beside it would
/// reintroduce the vendor-specific icon set this status model replaces.
/// What an attached session's row is called.  On Linux and WSL an attach is
/// full passthrough, so the pane on screen is herdr's and the row names it
/// the way herdr's own listed row would — attaching must not rename the row
/// under the user.  A pane herdr reports no title for keeps the title its own
/// PTY set, with the kind in front of it.
pub(super) fn session_row_name(
    pty_title: &str,
    activity: SessionActivity,
    agent: Option<&herdr::Agent>,
) -> RowName {
    let Some(agent) = agent else {
        return RowName::plain(session_row_title(pty_title, activity));
    };
    herdr_row_name(agent).unwrap_or_else(|| RowName {
        text: session_row_title(pty_title, activity),
        context: agent.kind.clone(),
    })
}

pub(super) fn session_row_title(title: &str, activity: SessionActivity) -> String {
    if activity.is_agent() {
        let trimmed = title.trim_start();
        if let Some(first) = trimmed.chars().next() {
            let rest = &trimmed[first.len_utf8()..];
            if !first.is_ascii() && rest.chars().next().is_some_and(char::is_whitespace) {
                let rest = rest.trim_start();
                if !rest.is_empty() {
                    return rest.to_string();
                }
            }
        }
    }
    title.to_string()
}

/// The "index. name" label for a profile entry in the worktree row's "Open
/// session" menu, 1-based to match `SpawnProfile1`..`SpawnProfile9` in the
/// palette.
pub(super) fn profile_menu_label(index: usize, name: &str) -> String {
    format!("{index}. {name}")
}

/// The liveness cache corrects discovery for paint and navigation only. Keep
/// this shared so a row that has just gone grey cannot remain a dead stop in
/// the workspace ring. Main checkouts are never prune candidates, even when
/// their project is a non-git directory with no .git entry.
pub(super) fn worktree_looks_gone(wt: &Worktree, missing: Option<bool>) -> bool {
    missing.map_or(wt.prunable, |gone| gone && !wt.is_main)
}

pub(super) fn worktree_is_switchable(
    wt: &Worktree,
    missing: Option<bool>,
    has_sessions: bool,
) -> bool {
    !worktree_looks_gone(wt, missing) || has_sessions
}

/// The workspaces a herdr agent may be matched against.  A checkout that
/// looks gone offers none, which is what makes an agent working there
/// unmatched rather than parked under a row that can only refuse it.
/// `missing` is the liveness cache's word for a path, `None` where it has
/// none, so the row's grey and this list agree about the same directory.
pub(super) fn herdr_workspaces(
    projects: &[Project],
    missing: impl Fn(&Path) -> Option<bool>,
) -> Vec<PathBuf> {
    projects
        .iter()
        .flat_map(|p| p.worktrees.iter())
        .filter(|wt| !worktree_looks_gone(wt, missing(&wt.path)))
        .map(|wt| wt.path.clone())
        .collect()
}

pub(super) struct HerdrRowAction {
    pub(super) attach: bool,
}

/// What a harness-managed row explains on hover, one fact per comma: the
/// state, since that is what changes; who reports it; whether the attach is
/// the harness's whole view; and what the harness calls the pane.  The way
/// out follows in parentheses, since it is an instruction rather than
/// another fact about the pane.
///
/// The same sentence serves a listed agent and an attached one.  Attaching
/// changes how alacritree draws a pane, not what there is to say about it,
/// and the chord has no other surface in alacritree — it is the harness's
/// key, not one of ours — so it has to reach the row the user is sitting in.
pub(super) fn managed_tooltip(managed: &Managed) -> String {
    let mut parts = Vec::new();
    if let Some(mark) = managed.mark {
        parts.push(mark.label.to_owned());
    }
    parts.push(managed.harness.to_owned());
    if managed.shared_view {
        parts.push("shared view".to_owned());
    }
    parts.extend(managed.pane_name());
    let mut hint = parts.join(", ");
    hint.push('.');
    if let Some(chord) = &managed.detach {
        // Backquoted because the chord is a sequence, not one combination:
        // unquoted, "detach with Ctrl+B q" reads as a sentence whose last
        // word happens to be `q`.
        hint.push_str(&format!(" (detach with `{chord}`)"));
    }
    hint
}

/// A shared-view attach waiting on herdr.  The gesture answers with the argv
/// its client runs, so everything the session needs is in hand by the time it
/// opens.
pub(super) struct PendingHerdrAttach {
    pub(super) job: Option<jobs::Job<Result<Launch, String>>>,
    /// The pane to focus once the gesture runs.  A pane the listing has since
    /// dropped is focused as this said, since nothing newer says otherwise.
    pub(super) target: PaneTarget,
    pub(super) key: herdr::HerdrKey,
    pub(super) workspace: WorkspaceKey,
    /// Where to hand the user back when herdr refuses.  A shared-view
    /// attach answers frames after the switch, so the caller cannot restore
    /// the workspace itself the way a direct attach lets it.
    pub(super) previous: WorkspaceKey,
    /// Clients parked on this attach.  A shared-view attach opens its session
    /// frames after the request that asked for it, so there is nothing to
    /// answer with until `poll_herdr_attach` resolves.
    pub(super) waiters: Vec<mpsc::Sender<ipc::IpcResult>>,
}

/// A pane being created.  The attach it turns into is the ordinary one, so
/// this queue only carries the gesture: `poll_herdr_create` hands the pane it
/// names to `attach_herdr_agent` and stops there.
///
/// One waiter, not a list: nothing merges two creates, since the pane they
/// would be merged on has no identity until herdr answers.
pub(super) struct PendingHerdrCreate {
    pub(super) job: jobs::Job<Result<CreatedPane, String>>,
    pub(super) side: herdr::Side,
    pub(super) workspace: WorkspaceKey,
    pub(super) waiter: Option<mpsc::Sender<ipc::IpcResult>>,
}

/// A pane the listing no longer carries.  Claiming an agent is in it keeps
/// every caller on the path it took before the pane went, which is what
/// `herdr_pane_has_agent` answers for the same reason.
pub(super) fn unlisted_pane_target(key: &herdr::HerdrKey, pane_id: &str) -> PaneTarget {
    PaneTarget {
        side: key.side.clone(),
        pane_id: pane_id.to_string(),
        tab_id: None,
        has_agent: true,
    }
}

/// Whether ending a session asks first.  A harness-managed one is a detach
/// rather than a kill, so it answers to its own switch: the attach client is
/// always running, which would make the busy question a close asks fire every
/// time and warn about nothing.
pub(super) fn close_needs_prompt(ui: &UiTheme, managed: bool, busy: bool) -> bool {
    if managed { ui.confirm_session_detach } else { ui.confirm_session_close.requires_prompt(busy) }
}

/// What the × on a session row does.  Ending a harness-managed session ends
/// the attach client and nothing else — the pane keeps running under the
/// harness, and the row it came from comes back — so calling that a close
/// promises a destruction that does not happen.
pub(super) fn close_button_hint(managed: bool) -> &'static str {
    if managed { "detach session" } else { "close session" }
}

/// The known worktree that owns `path`: the longest worktree path that
/// `path` equals or descends from.  Longest wins so a worktree nested under
/// another checkout resolves to the inner one.
pub(super) fn owning_worktree(worktrees: &[PathBuf], path: &Path) -> Option<PathBuf> {
    worktrees
        .iter()
        .filter(|wt| path.starts_with(wt))
        .max_by_key(|wt| wt.components().count())
        .cloned()
}

pub(super) fn unknown_worktree(path: &Path) -> String {
    format!("{} is not a worktree in the sidebar — see list_projects", path.display())
}

/// A name that reaches no server, as opposed to one whose server is down: a
/// caller retrying this one is retrying a typo.
pub(super) fn not_a_side(name: &str) -> String {
    format!("`{name}` is not a side, expected `native` or `wsl:<distro>`")
}

/// The directory a new pane opens in, spelled where the multiplexer resolves
/// it: the distro's own path on a WSL side, the Windows path on the native
/// one.  `None` leaves the choice to the multiplexer.  A workspace with no
/// spelling inside the distro is an `Err`, since a pane opened anywhere else
/// would still have its session filed under that workspace.
pub(super) fn multiplexer_cwd(
    side: &herdr::Side,
    workspace: Option<&Path>,
) -> Result<Option<String>, String> {
    let Some(path) = workspace else { return Ok(None) };
    match side {
        herdr::Side::Native => Ok(Some(path.display().to_string())),
        herdr::Side::Wsl(distro) => wsl::windows_to_linux(path)
            .map(Some)
            .ok_or_else(|| format!("{} has no path inside the {distro} distro", path.display())),
    }
}

/// Where a herdr pane lives, in the fields an attach takes back.  `pane_id`
/// and `tab_id` are null when the listing does not carry the pane, which
/// says the cache does not know right now rather than that the pane is
/// gone.
pub(super) fn multiplexer_json(
    side: &herdr::Side,
    terminal_id: &str,
    session: Option<String>,
    pane: Option<&herdr::Agent>,
) -> Value {
    json!({
        "name": "herdr",
        "side": side.name(),
        "session": session,
        "terminal_id": terminal_id,
        "pane_id": pane.map(|pane| pane.pane_id.clone()),
        "tab_id": pane.and_then(|pane| pane.tab_id.clone()),
    })
}

/// `SessionActivity` as the reply spells it.  A plain shell is null rather
/// than an object, so a consumer testing for presence needs no second field.
pub(super) fn activity_json(activity: SessionActivity) -> Value {
    match activity {
        SessionActivity::Shell => Value::Null,
        SessionActivity::Agent { name, live } => json!({
            "name": name,
            "state": live.label(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command_palette::PaletteItem;
    use crate::test_util::titled_herdr_agent as titled;

    const LIVE: SessionFocus = SessionFocus { scratchpad: false, exited: false };
    const EXITED: SessionFocus = SessionFocus { scratchpad: false, exited: true };
    const SCRATCHPAD: SessionFocus = SessionFocus { scratchpad: true, exited: false };

    /// The multiplexer resolves the directory where it runs, so a WSL side is
    /// handed the distro's own spelling of the workspace and never the
    /// Windows path the sidebar holds.
    #[cfg(windows)]
    #[test]
    fn a_new_pane_opens_in_the_workspace_spelled_for_its_own_side() {
        let workspace = PathBuf::from(r"\\wsl.localhost\ubuntu\home\dev\repo");
        assert_eq!(
            multiplexer_cwd(&herdr::Side::Wsl("ubuntu".into()), Some(&workspace)),
            Ok(Some("/home/dev/repo".to_string()))
        );
        assert_eq!(
            multiplexer_cwd(&herdr::Side::Native, Some(&workspace)),
            Ok(Some(workspace.display().to_string()))
        );
    }

    /// The home workspace names no directory, so herdr picks its own default
    /// rather than being handed an empty path.
    #[test]
    fn a_new_pane_in_the_home_workspace_names_no_directory() {
        assert_eq!(multiplexer_cwd(&herdr::Side::Native, None), Ok(None));
        assert_eq!(multiplexer_cwd(&herdr::Side::Wsl("ubuntu".into()), None), Ok(None));
    }

    /// The "workspaces" depth never reaches a child, whatever the query:
    /// `child_matches` is not built, so `current_project_rows` feeds
    /// `sidebar_nav::filtered_rows` a `None` child predicate and a query
    /// naming a session matches only that session's workspace.
    #[test]
    fn search_reaches_children_stays_false_at_the_workspaces_default() {
        assert!(!search_reaches_children(SearchDepth::Workspaces, false));
        assert!(!search_reaches_children(SearchDepth::Workspaces, true));
    }

    /// The "sessions" depth resolves child names for a non-empty query, so a
    /// session or agent row can match by its own name rather than only
    /// through its workspace.
    #[test]
    fn search_reaches_children_only_with_sessions_depth_and_a_live_query() {
        assert!(search_reaches_children(SearchDepth::Sessions, false));
        assert!(!search_reaches_children(SearchDepth::Sessions, true));
    }

    #[test]
    fn spawn_geometry_prefers_the_active_session_over_the_last_painted_pane() {
        let active = Some(ActiveGeometry {
            size: TermSize::new(120, 40),
            cell_size: (9.0, 18.0),
            is_scratchpad: false,
        });
        let last_pane = Some((TermSize::new(80, 24), (8.0, 16.0)));

        let (size, cell_size) = spawn_geometry(active, last_pane);

        assert_eq!((size.columns, size.screen_lines), (120, 40));
        assert_eq!(cell_size, (9.0, 18.0));
    }

    #[test]
    fn an_active_scratchpad_does_not_shadow_the_last_painted_pane() {
        // The size every scratchpad keeps for its whole life.
        let active = Some(ActiveGeometry {
            size: TermSize::new(80, 24),
            cell_size: (8.0, 16.0),
            is_scratchpad: true,
        });
        let last_pane = Some((TermSize::new(120, 40), (9.0, 18.0)));

        let (size, cell_size) = spawn_geometry(active, last_pane);

        assert_eq!((size.columns, size.screen_lines), (120, 40));
        assert_eq!(cell_size, (9.0, 18.0));
    }

    #[test]
    fn spawn_geometry_falls_back_to_the_last_painted_pane_without_an_active_session() {
        let last_pane = Some((TermSize::new(120, 40), (9.0, 18.0)));

        let (size, cell_size) = spawn_geometry(None, last_pane);

        assert_eq!((size.columns, size.screen_lines), (120, 40));
        assert_eq!(cell_size, (9.0, 18.0));
    }

    #[test]
    fn spawn_geometry_falls_back_to_80x24_before_anything_has_painted() {
        let (size, cell_size) = spawn_geometry(None, None);

        assert_eq!((size.columns, size.screen_lines), (80, 24));
        assert_eq!(cell_size, (8.0, 16.0));
    }

    #[test]
    fn the_visible_session_holds_the_self_boost_while_its_pty_is_still_opening() {
        // Nothing to raise yet, so `set_priority_boost` answered false.
        let visible = SessionBoost { raised: false, visible: true, pending: true };

        assert!(holds_self_boost(visible));
    }

    #[test]
    fn a_background_session_still_opening_its_pty_holds_no_self_boost() {
        let background = SessionBoost { raised: false, visible: false, pending: true };

        assert!(!holds_self_boost(background));
    }

    #[test]
    fn a_session_whose_job_took_the_boost_holds_it_wherever_it_sits() {
        let background = SessionBoost { raised: true, visible: false, pending: false };

        assert!(holds_self_boost(background));
    }

    #[test]
    fn a_frame_whose_visible_session_is_still_pending_leaves_the_self_boost_where_it_was() {
        let frame = [
            SessionBoost { raised: false, visible: false, pending: false },
            // On screen, its PTY still opening: no job exists to answer for
            // it, and the boost must survive the gap until one does.
            SessionBoost { raised: false, visible: true, pending: true },
            SessionBoost { raised: false, visible: false, pending: true },
        ];

        assert!(frame_holds_self_boost(frame.into_iter()));
    }

    #[test]
    fn a_frame_of_idle_background_sessions_drops_the_self_boost() {
        let frame =
            [SessionBoost { raised: false, visible: false, pending: false }, SessionBoost {
                raised: false,
                visible: false,
                pending: true,
            }];

        assert!(!frame_holds_self_boost(frame.into_iter()));
    }

    #[test]
    fn a_grey_worktree_only_stays_in_the_workspace_ring_while_it_holds_sessions() {
        let wt = Worktree {
            name: "gone".into(),
            path: PathBuf::from("/repo-worktrees/gone"),
            branch: Some("feature".into()),
            is_main: false,
            prunable: false,
            upstream: None,
        };

        assert!(!worktree_is_switchable(&wt, Some(true), false));
        assert!(worktree_is_switchable(&wt, Some(true), true));
    }

    #[test]
    fn a_main_checkout_never_looks_prunable_from_the_row_probe() {
        let wt = Worktree {
            name: "main".into(),
            path: PathBuf::from("/plain-project"),
            branch: None,
            is_main: true,
            prunable: false,
            upstream: None,
        };

        assert!(!worktree_looks_gone(&wt, Some(true)));
    }

    /// Apply `walk_swaps` to a concrete list, with `indices` standing in for
    /// the absolute slots one workspace occupies inside the session vector.
    fn walked(items: &[&str], indices: &[usize], j: usize, position: usize) -> Vec<String> {
        let mut v: Vec<String> = items.iter().map(|s| s.to_string()).collect();
        for (a, b) in walk_swaps(indices, j, position) {
            v.swap(a, b);
        }
        v
    }

    #[test]
    fn the_sentinel_sees_a_same_workspace_session_switch() {
        let written =
            SidebarFocusWrite { cursor: Some(SidebarRow::Home), workspace: None, active: Some(1) };
        let written = Some(written);

        // The reconciler's own values still stand.
        assert!(!sidebar_focus_overtaken(&written, Some(&SidebarRow::Home), &None, Some(1)));

        // Any action that switches sessions without leaving the workspace —
        // SelectNextTab, SelectNextSession, SelectTab(n) — changes neither the
        // cursor nor the workspace, only the active session.
        assert!(sidebar_focus_overtaken(&written, Some(&SidebarRow::Home), &None, Some(2)));

        // A different workspace, and a different cursor, each count too.
        assert!(sidebar_focus_overtaken(
            &written,
            Some(&SidebarRow::Home),
            &Some(PathBuf::from("/a/wt1")),
            Some(1),
        ));
        assert!(sidebar_focus_overtaken(
            &written,
            Some(&SidebarRow::Project(PathBuf::from("/a"))),
            &None,
            Some(1),
        ));

        // Nothing written yet cannot have been overtaken.
        assert!(!sidebar_focus_overtaken(&None, Some(&SidebarRow::Home), &None, Some(1)));
    }

    #[test]
    fn move_target_is_a_no_op_when_position_is_unchanged() {
        // Dropping above your own row, or just below it, changes nothing.
        assert_eq!(move_target(3, 1, 1), None);
        assert_eq!(move_target(3, 1, 2), None);
        // Dropping onto yourself.
        assert_eq!(move_target(3, 0, 0), None);
        // A stale source index (list shrank mid-drag) is ignored.
        assert_eq!(move_target(2, 5, 0), None);
    }

    #[test]
    fn walk_swaps_moves_within_a_contiguous_workspace() {
        assert_eq!(walked(&["a", "b", "c"], &[0, 1, 2], 0, 2), vec!["b", "c", "a"]);
        assert_eq!(walked(&["a", "b", "c"], &[0, 1, 2], 2, 0), vec!["c", "a", "b"]);
    }

    #[test]
    fn walk_swaps_leaves_interleaved_workspaces_in_place() {
        // Slots 0 and 2 belong to one workspace, slot 1 to another; moving the
        // first workspace's second session to the front must not disturb it.
        assert_eq!(walked(&["a", "x", "b"], &[0, 2], 1, 0), vec!["b", "x", "a"]);
    }

    #[test]
    fn walk_swaps_is_empty_when_nothing_moves() {
        assert!(walk_swaps(&[0, 1, 2], 1, 1).is_empty());
        // A position past the end clamps to the last slot, which is a no-op
        // for the element already there.
        assert!(walk_swaps(&[0, 1, 2], 2, 9).is_empty());
    }

    #[test]
    fn a_cross_workspace_drop_takes_the_display_slot_as_the_position() {
        // The session is not in that workspace's list yet, so nothing shifts
        // down and every slot passes through — including the two the same
        // workspace answers differently, which is what tells the branches
        // apart.
        assert_eq!(drop_position(false, 3, 1, 2), Some(2));
        assert_eq!(drop_position(false, 3, 1, 3), Some(3));
        // A drop onto the front of a workspace whose rows are all below it.
        assert_eq!(drop_position(false, 0, 0, 0), Some(0));
    }

    #[test]
    fn walk_swaps_places_an_arrival_at_the_stated_position() {
        // Arriving from another workspace, the display slot is the position:
        // nothing was removed from this list first, so there is no off-by-one.
        assert_eq!(walk_swaps(&[0, 1, 2], 2, 0), vec![(1, 2), (0, 1)]);
    }

    #[test]
    fn reorder_subject_prefers_the_cursored_session() {
        assert_eq!(
            reorder_subject(true, Some(&SidebarRow::Session(7)), || None, |_| None, || Some(3)),
            Some(7)
        );
    }

    #[test]
    fn reorder_subject_takes_a_workspace_rows_active_session() {
        // The landing after a cross-workspace step: the session paints no row
        // yet, so the cursor sits on the worktree it arrived in.
        let row = SidebarRow::Worktree(PathBuf::from("/b"));
        assert_eq!(
            reorder_subject(
                true,
                Some(&row),
                || None,
                |p| (p == Path::new("/b")).then_some(9),
                || Some(3)
            ),
            Some(9)
        );
        assert_eq!(
            reorder_subject(true, Some(&SidebarRow::Home), || Some(4), |_| None, || Some(3)),
            Some(4)
        );
    }

    #[test]
    fn reorder_subject_falls_back_to_the_session_on_screen() {
        // Terminal focused: the cursor is ignored entirely.
        assert_eq!(
            reorder_subject(false, Some(&SidebarRow::Session(7)), || None, |_| None, || Some(3)),
            Some(3)
        );
        // Sidebar focused on a project header, which owns no session.
        let row = SidebarRow::Project(PathBuf::from("/a"));
        assert_eq!(reorder_subject(true, Some(&row), || None, |_| None, || Some(3)), Some(3));
        // And an empty workspace row falls through rather than refusing.
        let row = SidebarRow::Worktree(PathBuf::from("/b"));
        assert_eq!(reorder_subject(true, Some(&row), || None, |_| None, || Some(3)), Some(3));
    }

    fn entries(ids: &[SessionId]) -> Vec<sidebar_nav::WorkspaceEntry> {
        ids.iter().copied().map(sidebar_nav::WorkspaceEntry::Session).collect()
    }

    #[test]
    fn workspace_entries_keep_shell_sessions_in_spawn_order() {
        assert_eq!(workspace_entries(&[1, 3], Vec::new(), false), entries(&[1, 3]));
    }

    #[test]
    fn profile_menu_label_numbers_from_one() {
        assert_eq!(profile_menu_label(1, "WSL"), "1. WSL");
        assert_eq!(profile_menu_label(2, "cmd"), "2. cmd");
    }

    #[test]
    fn the_status_hint_names_the_agent_and_what_it_is_doing() {
        assert_eq!(agent_hint(LiveState::Idle, Some("claude")), "claude is running");
        assert_eq!(agent_hint(LiveState::Working, Some("codex")), "codex is working");
        assert_eq!(agent_hint(LiveState::Blocked, Some("claude")), "claude is waiting for you");
        assert_eq!(agent_hint(LiveState::Blocked, None), "agent is waiting for you");
    }

    /// A shell session has no state axis to mark. The sidebar draws its own
    /// icon here instead of a status mark, and the palette leaves the slot
    /// empty, so both must read this as "no mark" rather than picking one.
    #[test]
    fn session_status_mark_picks_none_for_a_shell() {
        let status =
            RowStatus { attention: false, activity: SessionActivity::Shell, managed: None };
        assert!(session_status_mark(&status).is_none());
    }

    /// The palette paints the identical mark and hover the sidebar would for
    /// a local agent, whichever of the three live states it is in.
    #[test]
    fn session_status_mark_picks_each_live_state_for_a_local_agent() {
        for live in [LiveState::Idle, LiveState::Working, LiveState::Blocked] {
            let activity = SessionActivity::agent(Some("claude"), live);
            let status = RowStatus { attention: false, activity, managed: None };
            let (mark, hint) = session_status_mark(&status).expect("an agent always has a mark");
            assert_eq!(mark, SessionMark::Agent(live));
            assert_eq!(hint, agent_hint(live, Some("claude")));
        }
    }

    /// A herdr pane with no agent in it reports no state, so the harness rung
    /// of the ladder is empty and a plain shell in one carries no mark at all.
    /// The palette reads `managed.mark` directly while the sidebar goes
    /// through the ladder, so the two only agree while both answer "none"
    /// here.
    #[test]
    fn session_status_mark_leaves_an_agentless_pane_unmarked() {
        let managed = Managed::herdr(
            &herdr::Side::Native,
            &herdr::Settings::default(),
            AttachMode::Agent,
            Some(&shell_pane()),
        );
        assert_eq!(managed.mark, None);
        let status = RowStatus {
            attention: false,
            activity: SessionActivity::Shell,
            managed: Some(&managed),
        };
        assert!(session_status_mark(&status).is_none());
    }

    /// A pane herdr reports no agent in can still be running one alacritree's
    /// own title heuristic recognises.  The harness rung is empty, so the
    /// ladder falls through to the live axis rather than stopping at a
    /// managed row the way it did while every listed pane had a state.
    #[test]
    fn an_agentless_pane_falls_through_to_the_local_agent_reading() {
        let managed = Managed::herdr(
            &herdr::Side::Native,
            &herdr::Settings::default(),
            AttachMode::Agent,
            Some(&shell_pane()),
        );
        let activity = SessionActivity::agent(Some("claude"), LiveState::Working);
        assert_eq!(herdr_backed_activity(activity, None), activity);
        let status = RowStatus { attention: false, activity, managed: Some(&managed) };
        let (mark, hint) = session_status_mark(&status).expect("the live axis still has one");
        assert_eq!(mark, SessionMark::Agent(LiveState::Working));
        assert_eq!(hint, agent_hint(LiveState::Working, Some("claude")));
    }

    #[test]
    fn an_attached_herdr_session_takes_herdrs_live_state() {
        let claude = SessionActivity::agent(Some("claude"), LiveState::Idle);

        // Not attached to herdr: nothing overrides the session's own reading.
        assert_eq!(herdr_backed_activity(claude, None), claude);

        // herdr sees the approval dialog no title heuristic can.
        assert_eq!(
            herdr_backed_activity(claude, Some(herdr::Status::Blocked)),
            SessionActivity::agent(Some("claude"), LiveState::Blocked)
        );

        // An attached pane holds an agent even when the process probe missed
        // one, so the gate closes on herdr's word alone.
        assert_eq!(
            herdr_backed_activity(SessionActivity::Shell, Some(herdr::Status::Working)),
            SessionActivity::agent(None, LiveState::Working)
        );

        // `unknown` is herdr declining to say, not a claim of idleness: the
        // session keeps whatever it already knew.
        let working = SessionActivity::agent(Some("claude"), LiveState::Working);
        assert_eq!(herdr_backed_activity(working, Some(herdr::Status::Unknown)), working);
    }

    /// herdr distinguishes four live states and says so on its own panes.
    /// Collapsing any pair onto one mark would make the sidebar say less
    /// about a pane than the window it came from.
    #[test]
    fn herdr_marks_keep_its_four_states_apart() {
        for set in [herdr::Indicators::Dots, herdr::Indicators::Symbols] {
            let marks: Vec<HarnessMark> = [
                herdr::Status::Blocked,
                herdr::Status::Working,
                herdr::Status::Done,
                herdr::Status::Idle,
            ]
            .into_iter()
            .map(|status| herdr_mark(status, set))
            .collect();
            for (i, a) in marks.iter().enumerate() {
                for b in &marks[i + 1..] {
                    assert_ne!(a, b, "{set:?} draws two states the same");
                }
            }
        }
    }

    /// Taken from herdr's own `state_icon_symbol`, so a pane carries one mark
    /// whether it is read in herdr or in the sidebar.
    #[test]
    fn herdr_marks_are_the_ones_herdr_paints() {
        let dots = |status| herdr_mark(status, herdr::Indicators::Dots).glyph;
        assert_eq!(dots(herdr::Status::Blocked), "●");
        assert_eq!(dots(herdr::Status::Working), "●");
        assert_eq!(dots(herdr::Status::Done), "●");
        assert_eq!(dots(herdr::Status::Idle), "○");

        let symbols = |status| herdr_mark(status, herdr::Indicators::Symbols).glyph;
        assert_eq!(symbols(herdr::Status::Blocked), "×");
        assert_eq!(symbols(herdr::Status::Working), "◐");
        assert_eq!(symbols(herdr::Status::Done), "✓");
        assert_eq!(symbols(herdr::Status::Idle), "○");
    }

    /// A status alacritree does not recognise is herdr declining to say, and
    /// the row says that rather than claiming the agent is idle.
    #[test]
    fn an_unknown_herdr_status_is_drawn_as_no_reading() {
        for set in [herdr::Indicators::Dots, herdr::Indicators::Symbols] {
            let mark = herdr_mark(herdr::Status::Unknown, set);
            assert_eq!(mark.glyph, "·");
            assert_eq!(mark.tone, StateTone::Unclear);
        }
    }

    /// A herdr pane running a plain shell.  herdr names no agent in it, so
    /// the only thing it can be called is the title it set itself.
    fn shell_pane() -> herdr::Agent {
        herdr::Agent { status: None, ..titled(None, Some("~/G/g/alacritree")) }
    }

    /// A shell pane has no kind to fall back to, and six characters of a
    /// terminal id name nothing a user would recognise.
    #[test]
    fn an_agentless_pane_is_named_by_its_title() {
        assert_eq!(herdr_display_name(&shell_pane()), RowName::plain("~/G/g/alacritree".into()));
    }

    /// `unknown` is herdr's word for an agent it cannot classify, so a shell
    /// wearing it would claim an agent is there.
    #[test]
    fn an_agentless_pane_claims_no_status() {
        let agent = shell_pane();
        let content = herdr_palette_content(
            agent.title.clone(),
            &agent,
            Some("alacritree / master"),
            "◆",
            PathStyle::Fish,
            None,
        );
        assert_eq!(
            (content.primary, content.subtitle, content.secondary),
            ("~/G/g/alacritree".into(), "◆ alacritree / master".into(), "herdr · shell".into(),)
        );
    }

    /// The pane is still herdr's, which is what the row's mark says; the
    /// state is the part there is nothing to report.  Every `herdr agent`
    /// subcommand resolves its target through the agent registry, so the
    /// attach shares herdr's view even on a side that attaches directly.
    #[test]
    fn an_agentless_pane_paints_no_state_and_shares_the_view() {
        let row = HerdrRowData::from_agent(
            &shell_pane(),
            &herdr::Side::Wsl("d".into()),
            &herdr::Settings::default(),
            AttachMode::Agent,
        );
        assert_eq!(row.managed.mark, None);
        assert!(row.managed.shared_view);
        assert_eq!(managed_tooltip(&row.managed), r#"herdr, shared view, "~/G/g/alacritree"."#);
    }

    #[test]
    fn herdr_display_name_keeps_a_short_terminal_id_whole() {
        // `saturating_sub(6)` exists precisely for ids shorter than the tail
        // it takes; a plain `- 6` would panic on this one.
        let agent = herdr::Agent {
            terminal_id: "t1".into(),
            pane_id: "w1:p1".into(),
            tab_id: Some("w1:t1".into()),
            kind: None,
            title: None,
            status: Some(herdr::Status::Idle),
            focused: false,
            cwd: None,
            foreground_cwd: None,
        };
        assert_eq!(herdr_display_name(&agent), RowName::plain("t1".into()));
    }

    /// The filter matches what the row paints, so a query naming the category
    /// in front of an identity finds the row that shows both.
    #[test]
    fn search_text_carries_the_context_behind_the_identity() {
        let name = RowName { text: "primary".into(), context: Some("claude".into()) };
        assert_eq!(name.search_text(), "primary claude");
    }

    /// A row whose identity is already its category paints one word, so the
    /// filter searches one word rather than the same word twice.
    #[test]
    fn search_text_of_a_plain_name_is_the_name() {
        assert_eq!(RowName::plain("claude".into()).search_text(), "claude");
    }

    /// The row never compares its title against its own path, so a title that
    /// reads like a directory is named no differently than one that does
    /// not — the workspace label still keeps the line under it.
    #[test]
    fn native_titles_naming_their_directory_keep_the_workspace_label() {
        let content = native_palette_content(
            "/repo/feature".into(),
            "◆ renamed / main".into(),
            Some("claude"),
            Some("idle"),
            "shell",
        );
        assert_eq!(content.primary, "/repo/feature");
        assert_eq!(content.subtitle, "◆ renamed / main");
    }

    /// Nothing but the workspace label is left to name a titleless row, and
    /// once it takes the first line the second would only repeat it.
    #[test]
    fn a_titleless_native_row_is_named_by_its_workspace() {
        let content =
            native_palette_content(String::new(), "renamed / main".into(), None, None, "shell");
        assert_eq!((content.primary, content.subtitle), ("renamed / main".into(), String::new()));
    }

    #[test]
    fn native_home_titles_reach_palette_items() {
        let content = native_palette_content(
            "claude".into(),
            "Home".into(),
            Some("claude"),
            Some("idle"),
            "shell",
        );
        let item = PaletteItem::session(
            1,
            content.primary,
            content.subtitle,
            content.secondary,
            "hover".into(),
            Some("claude"),
            None,
            false,
        );
        assert_eq!(item.primary, "claude");
        assert_eq!(item.subtitle.as_deref(), Some("Home"));
        assert_eq!(item.secondary, "claude · idle");
    }

    /// The kind is spelled out even when the title already carries it: two
    /// rows in one state must read the same, and one repeated word is a
    /// cheaper price than a column that changes shape per row.
    #[test]
    fn the_middle_column_spells_the_kind_out_beside_a_title_that_shares_it() {
        assert_eq!(session_middle(None, Some("claude"), Some("idle"), "shell"), "claude · idle");
    }

    /// A lead with nothing after it still names itself rather than falling
    /// through to the fallback: the row is herdr-backed whatever else is
    /// unknown about it.
    #[test]
    fn a_lead_survives_an_otherwise_empty_middle_column() {
        assert_eq!(session_middle(Some("herdr"), None, None, "shell"), "herdr · shell");
    }

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

    #[test]
    fn native_shells_keep_a_shell_middle_cell() {
        let content = native_palette_content("terminal".into(), "Home".into(), None, None, "shell");
        assert_eq!(content.secondary, "shell");
    }

    /// Every scratchpad row carries the same one-word title, so the workspace
    /// label under it is what tells one from another.
    #[test]
    fn a_scratchpad_reads_its_kind_over_its_workspace() {
        let content = native_palette_content(
            "scratchpad".into(),
            "◆ renamed / main".into(),
            Some("scratchpad"),
            None,
            "shell",
        );
        assert_eq!(content.primary, "scratchpad");
        assert_eq!(content.subtitle, "◆ renamed / main");
        assert_eq!(content.secondary, "scratchpad");
    }

    /// The click switched workspace before handing the gesture over, so a
    /// failure puts the user back where the click found them.
    #[test]
    fn a_failed_attach_hands_back_the_workspace_it_switched_from() {
        let switched_to = Some(PathBuf::from("/code/wt"));
        let previous = Some(PathBuf::from("/code/other"));
        assert_eq!(
            workspace_after_failed_attach(&switched_to, &switched_to, previous.clone()),
            previous
        );
    }

    /// The home tab is a workspace like any other, so an attach launched from
    /// it is restored to it rather than read as nothing to go back to.
    #[test]
    fn a_failed_attach_restores_the_home_tab() {
        let switched_to = Some(PathBuf::from("/code/wt"));
        assert_eq!(workspace_after_failed_attach(&switched_to, &switched_to, None), None);
    }

    /// herdr answers frames after the click, and a switch made in between is
    /// the user's own: restoring over it would pull them out of a workspace
    /// they chose.
    #[test]
    fn a_failed_attach_leaves_a_workspace_the_user_moved_to_alone() {
        let current = Some(PathBuf::from("/code/elsewhere"));
        let switched_to = Some(PathBuf::from("/code/wt"));
        assert_eq!(workspace_after_failed_attach(&current, &switched_to, None), current);
    }

    /// A session alacritree still holds open after herdr stopped listing its
    /// pane has no state and no name left to report, but it is still herdr's
    /// and the user still has to know how to leave it.
    #[test]
    fn an_unlisted_pane_still_says_how_to_leave() {
        let settings =
            herdr::Settings { detach: Some("Ctrl+B q".into()), ..herdr::Settings::default() };
        let managed =
            Managed::herdr(&herdr::Side::Wsl("d".into()), &settings, AttachMode::Agent, None);
        assert_eq!(managed_tooltip(&managed), "herdr. (detach with `Ctrl+B q`)");
    }

    /// Ending a herdr-managed session ends the attach and leaves the pane
    /// running, so the control cannot call itself a close.
    #[test]
    fn the_close_control_is_a_detach_on_a_managed_row() {
        assert_eq!(close_button_hint(true), "detach session");
        assert_eq!(close_button_hint(false), "close session");
    }

    #[test]
    fn workspace_entries_apply_the_two_row_threshold() {
        assert!(workspace_entries(&[], Vec::new(), false).is_empty());
        assert!(workspace_entries(&[1], Vec::new(), false).is_empty());
        assert_eq!(workspace_entries(&[1, 3], Vec::new(), false), entries(&[1, 3]));
    }

    #[test]
    fn workspace_entries_always_flag_lists_single_sessions() {
        assert_eq!(workspace_entries(&[1], Vec::new(), true), entries(&[1]));
        assert!(workspace_entries(&[], Vec::new(), true).is_empty());
    }

    #[test]
    fn fallback_goes_home_from_home() {
        assert_eq!(close_fallback(&None, &None, &[], None), CloseFallback::Home);
    }

    #[test]
    fn a_deferred_verdict_survives_instead_of_being_re_derived() {
        // `close_fallback` is the only thing that knows to hop to the project's
        // main checkout; a generic "spawn something" fallback would strand
        // last_session_close = "navigate" in the workspace that just emptied.
        let main = PathBuf::from("/p/main");
        let removed = Some(PathBuf::from("/p/feature"));
        let remaining = vec![(Some(main.clone()), 1)];

        let verdict = close_fallback(&removed, &removed, &remaining, Some(main.clone()));
        assert_eq!(verdict, CloseFallback::Activate(main.clone()));

        let deferred = DeferredClose { verdict, removed_worktree: None };
        assert_eq!(
            deferred.verdict,
            CloseFallback::Activate(main),
            "the verdict is carried, not recomputed from whatever state remains"
        );
    }

    /// A user's close navigates: away from an emptied workspace, or into a
    /// replacement shell.  A failed open must do neither.  Wherever it
    /// navigates to, `ensure_active_session` spawns into it, and that open
    /// fails the same way.
    #[test]
    fn a_failed_spawn_neither_navigates_nor_respawns() {
        assert_eq!(close_navigation(CloseReason::User, CloseFallback::Home), CloseFallback::Home);
        assert_eq!(
            close_navigation(CloseReason::SpawnFailed, CloseFallback::Home),
            CloseFallback::Stay
        );
    }

    #[test]
    fn only_follow_defers_close_navigation() {
        use crate::config::SidebarFocus;

        assert!(defers_close_navigation(SidebarFocus::Follow));
        assert!(!defers_close_navigation(SidebarFocus::Preserve));
    }

    /// Keyboard-originated `focus_move` with both panels open.
    fn mv(focus: PaneFocus, dir: FocusDir, tui_running: bool) -> FocusMove {
        focus_move(focus, dir, true, true, ActionOrigin::Keyboard, tui_running)
    }

    #[test]
    fn focus_moves_between_open_panels() {
        assert_eq!(
            mv(PaneFocus::Terminal, FocusDir::Left, false),
            FocusMove::Focus(PaneFocus::ProjectsSidebar)
        );
        assert_eq!(
            mv(PaneFocus::Terminal, FocusDir::Right, false),
            FocusMove::Focus(PaneFocus::GitSidebar)
        );
        assert_eq!(
            mv(PaneFocus::ProjectsSidebar, FocusDir::Right, false),
            FocusMove::Focus(PaneFocus::Terminal)
        );
        assert_eq!(
            mv(PaneFocus::GitSidebar, FocusDir::Left, false),
            FocusMove::Focus(PaneFocus::Terminal)
        );
    }

    #[test]
    fn focus_stops_at_the_outer_edges() {
        assert_eq!(mv(PaneFocus::ProjectsSidebar, FocusDir::Left, false), FocusMove::Nothing);
        assert_eq!(mv(PaneFocus::GitSidebar, FocusDir::Right, false), FocusMove::Nothing);
    }

    #[test]
    fn focus_never_moves_toward_a_closed_panel() {
        assert_eq!(
            focus_move(
                PaneFocus::Terminal,
                FocusDir::Left,
                false,
                true,
                ActionOrigin::Keyboard,
                false
            ),
            FocusMove::Nothing
        );
        assert_eq!(
            focus_move(
                PaneFocus::Terminal,
                FocusDir::Right,
                true,
                false,
                ActionOrigin::Keyboard,
                false
            ),
            FocusMove::Nothing
        );
    }

    #[test]
    fn running_tui_keeps_the_key() {
        assert_eq!(mv(PaneFocus::Terminal, FocusDir::Left, true), FocusMove::Passthrough);
        assert_eq!(mv(PaneFocus::Terminal, FocusDir::Right, true), FocusMove::Passthrough);
    }

    /// A palette-dispatched Focus Left/Right is a binding stand-in, so a
    /// running TUI must see the same passthrough a real keypress would.
    #[test]
    fn palette_origin_keeps_the_key_for_a_running_tui() {
        assert_eq!(
            focus_move(
                PaneFocus::Terminal,
                FocusDir::Left,
                true,
                true,
                ActionOrigin::Palette,
                true
            ),
            FocusMove::Passthrough
        );
    }

    #[test]
    fn sidebars_never_pass_through() {
        assert_eq!(
            mv(PaneFocus::ProjectsSidebar, FocusDir::Right, true),
            FocusMove::Focus(PaneFocus::Terminal)
        );
    }

    /// An IPC move is the inner program saying it is out of windows —
    /// passthrough would bounce the key straight back to it.
    #[test]
    fn ipc_moves_never_pass_through() {
        assert_eq!(
            focus_move(PaneFocus::Terminal, FocusDir::Left, true, true, ActionOrigin::Ipc, true),
            FocusMove::Focus(PaneFocus::ProjectsSidebar)
        );
        assert_eq!(
            focus_move(PaneFocus::Terminal, FocusDir::Left, false, true, ActionOrigin::Ipc, true),
            FocusMove::Nothing
        );
    }

    /// The terminal owning focus over a live session, which is what every
    /// scope test that does not say otherwise means.
    fn scope() -> BindingScope {
        BindingScope::default()
    }

    /// The mapping the filter chain cannot check for itself: which pane owns
    /// focus, and whether the session on screen still has a child.
    #[test]
    fn binding_scope_reads_focus_and_the_session_on_screen() {
        let terminal = |active| binding_scope(PaneFocus::Terminal, false, active);

        assert!(terminal(Some(EXITED)).exited_session_focused);
        assert!(!terminal(Some(LIVE)).exited_session_focused);
        assert!(!terminal(None).exited_session_focused);
        assert!(terminal(Some(SCRATCHPAD)).scratchpad_focused);
        assert!(!terminal(Some(LIVE)).scratchpad_focused);

        let sidebar = binding_scope(PaneFocus::ProjectsSidebar, false, Some(EXITED));
        assert!(sidebar.sidebar_focused);
        assert!(!sidebar.git_focused);
        assert!(
            !sidebar.exited_session_focused,
            "a session's chord is the terminal's, not the sidebar's"
        );

        let git = binding_scope(PaneFocus::GitSidebar, false, Some(EXITED));
        assert!(git.git_focused);
        assert!(!git.sidebar_focused);
        assert!(!git.exited_session_focused);
    }

    /// The palette owns every key while it is up, so no scope is live under it.
    #[test]
    fn an_open_palette_leaves_no_scope_active() {
        for focus in [PaneFocus::Terminal, PaneFocus::ProjectsSidebar, PaneFocus::GitSidebar] {
            let scope = binding_scope(focus, true, Some(EXITED));
            assert!(!scope.sidebar_focused, "{focus:?}");
            assert!(!scope.git_focused, "{focus:?}");
            assert!(!scope.scratchpad_focused, "{focus:?}");
            assert!(!scope.exited_session_focused, "{focus:?}");
        }
    }

    #[test]
    fn projects_filter_action_valid_when_projects_sidebar_focused() {
        let action = BindingAction::Named(NamedAction::ToggleSessionsFilter);
        assert!(valid_for_focus(&action, BindingScope { sidebar_focused: true, ..scope() }));
    }

    #[test]
    fn projects_filter_action_rejected_when_git_sidebar_focused() {
        let action = BindingAction::Named(NamedAction::ToggleSessionsFilter);
        assert!(!valid_for_focus(&action, BindingScope { git_focused: true, ..scope() }));
    }

    #[test]
    fn git_filter_action_valid_when_git_sidebar_focused() {
        let action = BindingAction::Named(NamedAction::ToggleModifiedFilter);
        assert!(valid_for_focus(&action, BindingScope { git_focused: true, ..scope() }));
    }

    #[test]
    fn git_filter_action_rejected_when_projects_sidebar_focused() {
        let action = BindingAction::Named(NamedAction::ToggleModifiedFilter);
        assert!(!valid_for_focus(&action, BindingScope { sidebar_focused: true, ..scope() }));
    }

    #[test]
    fn both_sidebar_filters_rejected_when_terminal_focused() {
        let projects_action = BindingAction::Named(NamedAction::ToggleSessionsFilter);
        let git_action = BindingAction::Named(NamedAction::ToggleModifiedFilter);
        assert!(!valid_for_focus(&projects_action, scope()));
        assert!(!valid_for_focus(&git_action, scope()));
    }

    /// `ScrollPageUp` is unscoped by pane focus, so only the scratchpad
    /// editor stealing it back (via `terminal_only`) should block it.
    #[test]
    fn terminal_only_action_yields_to_the_scratchpad_editor() {
        let action = BindingAction::Named(NamedAction::ScrollPageUp);
        assert!(!valid_for_focus(&action, BindingScope { scratchpad_focused: true, ..scope() }));
        assert!(valid_for_focus(&action, scope()));
    }

    /// The one that decides whether the terminal stays usable: `Enter` is the
    /// default trigger for `CloseExitedSession`, and bindings are consumed
    /// ahead of `event_to_bytes`, so dispatching anything here would take the
    /// key away from every shell prompt in the app.
    #[test]
    fn a_live_session_keeps_its_enter() {
        let bindings = crate::bindings::parse_bindings(Vec::new());
        let matched =
            crate::bindings::all_matches(&bindings, egui::Key::Enter, egui::Modifiers::NONE);
        assert!(
            matched
                .iter()
                .any(|a| matches!(a, BindingAction::Named(NamedAction::CloseExitedSession))),
            "Enter must still reach the exited-session binding"
        );
        assert!(
            dispatched_actions(matched, scope()).is_empty(),
            "a live session's Enter must fall through to the PTY"
        );
    }

    /// Once the child is gone the same press closes the session instead.
    #[test]
    fn an_exited_session_dispatches_enter_to_the_close_action() {
        let bindings = crate::bindings::parse_bindings(Vec::new());
        let matched =
            crate::bindings::all_matches(&bindings, egui::Key::Enter, egui::Modifiers::NONE);
        let scope = BindingScope { exited_session_focused: true, ..scope() };
        let dispatched = dispatched_actions(matched, scope);
        assert_eq!(dispatched.len(), 1, "{dispatched:?}");
        assert!(
            matches!(dispatched[0], BindingAction::Named(NamedAction::CloseExitedSession)),
            "{dispatched:?}"
        );
    }

    #[test]
    fn a_wide_search_stands_down_the_project_toggles() {
        // Toggled on, workspace fails both: excluded while the toggles apply,
        // included once a wide search stands them down.
        assert!(!project_toggles_pass(true, true, false, true, false));
        assert!(project_toggles_pass(false, true, false, true, false));
    }

    #[test]
    fn sessions_filter_counts_a_detached_agent_bucketed_under_home() {
        let listed =
            sidebar_nav::ListedRows::from([(None, vec![sidebar_nav::WorkspaceEntry::Agent(
                herdr::Side::Native,
                "term_home".to_string(),
            )])]);
        assert!(sessions_filter_passes(&[], &listed, &None, true));
    }

    #[test]
    fn a_pr_toggle_alone_makes_any_toggle_active() {
        assert!(!any_project_toggle_active(false, false, false));
        assert!(any_project_toggle_active(false, false, true));
    }

    #[test]
    fn worktree_pr_passes_is_inert_without_a_pr_toggle() {
        let path = PathBuf::from("/worktree");
        let mut pr_matches = HashMap::new();
        pr_matches.insert(path.clone(), false);
        assert!(worktree_pr_passes(false, &pr_matches, &path));
    }

    #[test]
    fn worktree_pr_passes_follows_the_map_once_a_pr_toggle_is_active() {
        let path = PathBuf::from("/worktree");
        let mut pr_matches = HashMap::new();
        pr_matches.insert(path.clone(), true);
        assert!(worktree_pr_passes(true, &pr_matches, &path));
        pr_matches.insert(path.clone(), false);
        assert!(!worktree_pr_passes(true, &pr_matches, &path));
    }

    #[test]
    fn worktree_pr_passes_excludes_a_worktree_missing_from_the_map() {
        let path = PathBuf::from("/worktree");
        let pr_matches: HashMap<PathBuf, bool> = HashMap::new();
        assert!(!worktree_pr_passes(true, &pr_matches, &path));
    }

    #[test]
    fn ui_text_px_defaults_to_terminal_derivation() {
        let font = crate::config::FontConfig::default();
        let (normal, heading) = ui_text_px(&font, &crate::config::UiFont::default());
        assert_eq!(normal, font.ui_normal_px());
        assert_eq!(heading, font.ui_heading_px());
    }

    #[test]
    fn ui_text_px_overrides_from_ui_font_size() {
        let font = crate::config::FontConfig::default();
        let ui = crate::config::UiFont { size: Some(12.0), ..Default::default() };
        let (normal, heading) = ui_text_px(&font, &ui);
        assert_eq!(normal, 16.0); // 12 pt × 96/72
        assert_eq!(
            heading,
            16.0 * (crate::config::FontConfig::UI_HEADING_RATIO
                / crate::config::FontConfig::UI_NORMAL_RATIO)
        );
    }

    #[test]
    fn owning_worktree_matches_exact_and_descendant_paths() {
        let wts = vec![PathBuf::from("C:/w/feat-a"), PathBuf::from("C:/w/feat-b")];
        assert_eq!(
            owning_worktree(&wts, Path::new("C:/w/feat-a")),
            Some(PathBuf::from("C:/w/feat-a"))
        );
        assert_eq!(
            owning_worktree(&wts, Path::new("C:/w/feat-b/src/deep")),
            Some(PathBuf::from("C:/w/feat-b"))
        );
        assert_eq!(owning_worktree(&wts, Path::new("C:/elsewhere")), None);
    }

    /// A worktree checked out inside another checkout's subtree (e.g. under the
    /// main repo) must resolve to the inner worktree, not the enclosing one.
    #[test]
    fn owning_worktree_prefers_the_longest_prefix() {
        let wts = vec![PathBuf::from("C:/repo"), PathBuf::from("C:/repo/wt/inner")];
        assert_eq!(
            owning_worktree(&wts, Path::new("C:/repo/wt/inner/src")),
            Some(PathBuf::from("C:/repo/wt/inner"))
        );
    }

    /// The on-screen session keeps being watched: the view follows it to the
    /// target workspace.
    #[test]
    fn moving_the_on_screen_session_follows_it() {
        let out = plan_move(true, true, None, false);
        assert!(out.follow);
        assert!(out.claim_target);
        assert!(matches!(out.source, SourceRepair::Remove));
    }

    /// A background move is silent — no focus stealing — and only claims the
    /// target's active slot when the target had none.
    #[test]
    fn a_background_move_never_steals_focus() {
        let out = plan_move(false, false, None, true);
        assert!(!out.follow);
        assert!(!out.claim_target, "the target's own active session stays");
        assert!(matches!(out.source, SourceRepair::Keep));

        let out = plan_move(false, false, None, false);
        assert!(!out.follow);
        assert!(out.claim_target, "an empty target adopts the arrival");
    }

    /// Moving the source workspace's active-but-not-on-screen session promotes
    /// the next remaining session there, the way closing it would.
    #[test]
    fn the_source_workspace_repairs_its_active_session() {
        let out = plan_move(true, false, Some(9), false);
        assert!(matches!(out.source, SourceRepair::Set(9)));
        assert!(!out.follow);

        let out = plan_move(true, false, None, false);
        assert!(matches!(out.source, SourceRepair::Remove), "no session left to promote");
    }

    /// The job's spans, as `path_label` itself builds them via `zed_spans`,
    /// must reassemble into exactly what `render` produces, so the emphasis
    /// only changes how the text looks, never what it says.
    #[test]
    fn the_zed_job_spells_the_same_text_as_render() {
        for (path, home) in [
            ("path/to/file.txt", None),
            ("/a/b/c.txt", None),
            ("f.txt", None),
            ("/f.txt", None),
            ("/home/lev/Git/x/y.rs", Some("/home/lev")),
        ] {
            let parts = crate::path_style::split(path, PathStyle::Zed, home);
            let spans = zed_spans(&parts).concat();
            assert_eq!(spans, crate::path_style::render(path, PathStyle::Zed, home), "{path:?}");
        }
    }

    #[test]
    fn snapshot_parents_agree_with_the_row_model() {
        use crate::sidebar_focus::Parent;
        use crate::sidebar_nav::{self, SidebarRow};

        // Two projects, one collapsed, with sessions under the expanded one.
        let projects = vec![
            sidebar_nav::tests::project("/a", true, &["/a/wt1", "/a/wt2"]),
            sidebar_nav::tests::project("/b", false, &["/b/wt1"]),
        ];
        let live =
            vec![(None, 1), (Some(PathBuf::from("/a/wt1")), 2), (Some(PathBuf::from("/a/wt1")), 3)];
        let listed = sidebar_nav::tests::sessions_only(HashMap::from([
            (None, vec![1]),
            (Some(PathBuf::from("/a/wt1")), vec![2, 3]),
        ]));
        let rows = sidebar_nav::visible_rows(&projects, &listed);
        let snapshot =
            build_sidebar_snapshot(&projects, &live, &listed, &rows, None, Default::default());

        for row in &rows {
            let id = snapshot.find(row).expect("every projected row is in the model");
            let arena_parent = match snapshot.parent(id) {
                Parent::Root => None,
                Parent::Node(p) => Some(snapshot.row(p).clone()),
                Parent::Detached => panic!("a projected row is never detached: {row:?}"),
            };
            assert_eq!(
                arena_parent,
                sidebar_nav::left_target(&rows, row),
                "arena parent must agree with the row model for {row:?}"
            );
        }

        // The collapsed project's worktree is in the model but not projected.
        let hidden = snapshot
            .find(&SidebarRow::Worktree(PathBuf::from("/b/wt1")))
            .expect("collapsed worktrees stay in the model");
        assert!(!snapshot.is_projected(hidden));
    }

    #[test]
    fn a_session_below_the_listing_threshold_is_still_in_the_model() {
        use crate::sidebar_nav::{self, SidebarRow};

        let projects = vec![sidebar_nav::tests::project("/a", true, &["/a/wt1"])];
        // One live session in the worktree.  The real rule needs two before it
        // lists any, so this one is live but unprojected.
        let live = vec![(Some(PathBuf::from("/a/wt1")), 7)];
        let listed = {
            let mut l = sidebar_nav::ListedRows::new();
            let entries = workspace_entries(&[7], Vec::new(), false);
            assert!(entries.is_empty(), "the threshold rule must actually drop this session");
            if !entries.is_empty() {
                l.insert(Some(PathBuf::from("/a/wt1")), entries);
            }
            l
        };
        let rows = sidebar_nav::visible_rows(&projects, &listed);
        let snapshot =
            build_sidebar_snapshot(&projects, &live, &listed, &rows, None, Default::default());

        let id = snapshot
            .find(&SidebarRow::Session(7))
            .expect("a live session is in the model whatever the listing threshold says");
        assert!(!snapshot.is_projected(id), "but it is not a navigable row");
    }

    #[test]
    fn a_session_whose_project_is_gone_is_detached_not_deleted() {
        use crate::sidebar_focus::Parent;
        use crate::sidebar_nav::{self, SidebarRow};

        // `remove_project` drops the project but keeps its sessions running.
        let projects: Vec<crate::projects::Project> = vec![];
        let live = vec![(Some(PathBuf::from("/orphan/wt1")), 5)];
        let listed = sidebar_nav::ListedRows::new();
        let rows = sidebar_nav::visible_rows(&projects, &listed);
        let snapshot =
            build_sidebar_snapshot(&projects, &live, &listed, &rows, None, Default::default());

        let id = snapshot.find(&SidebarRow::Session(5)).expect("the session is still running");
        assert_eq!(
            snapshot.parent(id),
            Parent::Detached,
            "an orphan must not become a sibling of Home"
        );
    }

    #[test]
    fn a_worktree_being_deleted_reads_as_gone_immediately() {
        use crate::sidebar_nav::{self, SidebarRow};

        let projects = vec![sidebar_nav::tests::project("/a", true, &["/a/wt1", "/a/wt2"])];
        let listed = sidebar_nav::ListedRows::new();
        let rows = sidebar_nav::visible_rows(&projects, &listed);
        let doomed = PathBuf::from("/a/wt2");
        let snapshot = build_sidebar_snapshot(
            &projects,
            &[],
            &listed,
            &rows,
            Some(doomed.as_path()),
            Default::default(),
        );

        assert_eq!(
            snapshot.find(&SidebarRow::Worktree(doomed)),
            None,
            "the async git delete has not finished, but the row must not read as present"
        );
        assert!(snapshot.find(&SidebarRow::Worktree(PathBuf::from("/a/wt1"))).is_some());
    }

    /// The rows below a worktree being deleted must stay navigable.
    ///
    /// The projection is built before the deletion is known, so it still
    /// lists the doomed worktree.  The builder consumes that projection in
    /// lockstep, so skipping the worktree without stepping the index leaves
    /// it parked on a row nothing will ever match again — every later node
    /// reads as unprojected, and the cursor repair treats an unprojected row
    /// as one that has gone away.
    #[test]
    fn rows_below_a_deleted_worktree_stay_navigable() {
        use crate::sidebar_nav::{self, SidebarRow};

        let projects =
            vec![sidebar_nav::tests::project("/a", true, &["/a/wt1", "/a/wt2", "/a/wt3"])];
        let listed = sidebar_nav::ListedRows::new();
        let rows = sidebar_nav::visible_rows(&projects, &listed);
        let doomed = PathBuf::from("/a/wt2");
        let snapshot = build_sidebar_snapshot(
            &projects,
            &[],
            &listed,
            &rows,
            Some(doomed.as_path()),
            Default::default(),
        );

        let below = snapshot
            .find(&SidebarRow::Worktree(PathBuf::from("/a/wt3")))
            .expect("the worktree below the deleted one is still in the tree");
        assert!(
            snapshot.is_projected(below),
            "a row below the one being deleted must still be navigable"
        );
    }

    /// A checkout the liveness cache calls gone offers no workspace, so the
    /// agent working in it matches nothing and lists under Home.  Matched to
    /// the removed worktree instead, its row's Enter could only refuse.
    #[test]
    fn a_gone_worktree_offers_no_workspace_to_an_agent() {
        use crate::sidebar_nav;

        let projects = vec![sidebar_nav::tests::project("/a", true, &["/a/wt1", "/a/wt2"])];
        let gone = PathBuf::from("/a/wt2");
        let workspaces = herdr_workspaces(&projects, |path| Some(path == gone));
        assert_eq!(workspaces, vec![PathBuf::from("/a/wt1")]);

        let agent = herdr::Agent {
            terminal_id: "t1".into(),
            pane_id: "w1:p1".into(),
            tab_id: Some("w1:t1".into()),
            kind: None,
            title: None,
            status: Some(herdr::Status::Idle),
            focused: false,
            cwd: Some(gone.to_string_lossy().into_owned()),
            foreground_cwd: None,
        };
        assert_eq!(
            herdr::match_workspace(&agent, &herdr::Side::Native, &workspaces),
            None,
            "an agent under a removed checkout falls back to Home"
        );
    }

    /// The lockstep walk follows the listing, not the session vector.
    ///
    /// Attaching to the second pane first leaves the two sessions in the
    /// opposite order to herdr's, and a walk that trusted the vector would
    /// push them the wrong way round, match neither against the projection
    /// and trip its own assert.
    #[test]
    fn the_snapshot_walk_follows_the_listing_not_the_session_vector() {
        use crate::sidebar_nav::{self, SidebarRow};

        let projects = vec![sidebar_nav::tests::project("/a", true, &["/a/wt1"])];
        let wt = Some(PathBuf::from("/a/wt1"));
        // Attached in the order 9 then 4; herdr lists the panes 4 then 9.
        let live = vec![(wt.clone(), 9), (wt.clone(), 4)];
        let listed = sidebar_nav::ListedRows::from([(wt.clone(), vec![
            sidebar_nav::WorkspaceEntry::Session(4),
            sidebar_nav::WorkspaceEntry::Session(9),
        ])]);
        let rows = sidebar_nav::visible_rows(&projects, &listed);
        let snapshot =
            build_sidebar_snapshot(&projects, &live, &listed, &rows, None, Default::default());

        assert_eq!(rows, vec![
            SidebarRow::Home,
            SidebarRow::Project(PathBuf::from("/a")),
            SidebarRow::Worktree(PathBuf::from("/a/wt1")),
            SidebarRow::Session(4),
            SidebarRow::Session(9),
        ]);
        for row in &rows {
            let id = snapshot.find(row).expect("every projected row is in the model");
            assert!(snapshot.is_projected(id), "{row:?} must stay navigable");
        }
    }

    /// Dispatch cannot catch a wrong pairing: `toggle` drops an identity the
    /// panel does not allow, and an action with no arm falls through to the
    /// scroll handler.  Swapping two identities here is otherwise invisible.
    #[test]
    fn the_projects_filter_actions_map_to_their_identities() {
        for (action, identity) in [
            (NamedAction::ToggleSessionsFilter, Some('s')),
            (NamedAction::ToggleDetachedSessionsFilter, None),
            (NamedAction::ToggleAttentionFilter, Some('a')),
            (NamedAction::TogglePrOpenFilter, Some('o')),
            (NamedAction::TogglePrDraftFilter, Some('d')),
            (NamedAction::TogglePrMergedFilter, Some('m')),
            (NamedAction::TogglePrClosedFilter, Some('c')),
            (NamedAction::ClearProjectFilters, None),
            (NamedAction::ToggleModifiedFilter, None),
            (NamedAction::ToggleDeletedFilter, None),
            (NamedAction::ToggleUntrackedFilter, None),
            (NamedAction::ToggleSearchScope, None),
            (NamedAction::RefreshPrStatus, None),
            (NamedAction::Paste, None),
        ] {
            assert_eq!(project_filter_identity(action), identity, "{action:?}");
            if let Some(key) = identity {
                assert!(
                    project_filter_toggles(true).contains(&key),
                    "{action:?} maps to {key}, which the panel would drop"
                );
            }
        }
    }

    #[test]
    fn the_pr_identities_exist_only_when_polling_does() {
        assert_eq!(project_filter_toggles(false), &['s', 'a']);
        assert_eq!(project_filter_toggles(true), &['s', 'a', 'o', 'd', 'm', 'c']);
    }

    /// Guards the staging dependency: the four PR actions already dispatch to
    /// `project_filter.toggle`, and `toggle` silently ignores an identity the
    /// filter does not allow — so a narrow slice here makes them dead keys.
    #[test]
    fn the_pr_actions_reach_a_configured_projects_filter() {
        let mut f = PanelFilter::new(project_filter_toggles(true));
        for key in ['o', 'd', 'm', 'c'] {
            f.toggle(key);
            assert!(f.is_toggled(key), "{key} must be a live identity");
        }
    }

    #[test]
    fn any_pr_toggle_active_ignores_the_non_pr_identities() {
        let mut f = PanelFilter::new(project_filter_toggles(true));
        assert!(!any_pr_toggle_active(&f, SearchScope::Filtered));
        f.toggle('s');
        assert!(
            !any_pr_toggle_active(&f, SearchScope::Filtered),
            "a session toggle is not a PR toggle"
        );
        f.toggle('o');
        assert!(any_pr_toggle_active(&f, SearchScope::Filtered));
    }

    /// A search under `All` stands the toggles down for row selection, so the
    /// PR dimension narrows nothing — polling collapsed projects for it and
    /// rebuilding on every banked result would both be pure cost.
    #[test]
    fn a_stood_down_pr_toggle_does_not_read_as_active() {
        let mut f = PanelFilter::new(project_filter_toggles(true));
        f.toggle('o');
        f.on_text("/");
        f.on_text("a");

        assert!(any_pr_toggle_active(&f, SearchScope::Filtered));
        assert!(!any_pr_toggle_active(&f, SearchScope::All));
    }

    /// The reconciler must not churn for users who never touch a PR filter:
    /// every banked result would otherwise rebuild the row set.
    #[test]
    fn the_generation_reaches_the_reconciler_only_while_filtering() {
        assert_eq!(pr_generation_for(7, false), 0);
        assert_eq!(pr_generation_for(7, true), 7);
    }

    #[test]
    fn a_pr_filter_reaches_into_collapsed_projects() {
        assert!(!should_poll_pr(true, false, false), "collapsed and unfiltered: no lookup");
        assert!(should_poll_pr(true, false, true), "a PR filter must see collapsed rows");
        assert!(should_poll_pr(true, true, false));
        assert!(!should_poll_pr(false, true, true), "disabled means never");
    }

    #[test]
    fn the_palette_never_asks_for_a_negative_width() {
        assert!(palette_content_width(1.0, 10.0) >= 0.0);
    }

    /// A window wide enough keeps the fixed grid, so the columns line up exactly
    /// where they always have.
    #[test]
    fn wide_columns_keep_the_fixed_grid() {
        let cols = PaletteColumns::new(1.0, 760.0);
        assert_eq!(cols.action, 200.0);
        assert_eq!(cols.keys, 180.0);
        let mark = ROW_STATUS_ICON_W + PALETTE_MARK_GAP;
        assert_eq!(cols.desc, 760.0 - 2.0 * 10.0 - mark - 2.0 * 14.0 - 380.0);
        assert!(!cols.narrow, "a wide palette ellipsizes its columns rather than wrapping them");
    }

    #[test]
    fn only_a_cut_column_offers_its_full_text_on_hover() {
        assert_eq!(elided_hover(&[(false, "Copy"), (false, "Copy"), (false, "Ctrl+C")]), None);
        assert_eq!(
            elided_hover(&[
                (false, "Increase the font size"),
                (true, "IncreaseFontSize"),
                (true, "Ctrl+Plus, Ctrl+="),
            ]),
            Some("IncreaseFontSize\nCtrl+Plus, Ctrl+=".to_string())
        );
    }
}
