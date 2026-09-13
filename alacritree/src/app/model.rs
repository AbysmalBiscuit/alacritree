//! The data models and decision functions behind `AlacritreeApp`, split out
//! so they can be read and tested without the render pass around them. Nothing
//! here names an egui type: an item that paints belongs in the parent module.

use std::path::{Path, PathBuf};

use alacritty_terminal::tty::Shell;

use serde_json::{Value, json};

use crate::bindings::{BindingAction, KeyBinding, NamedAction};
use crate::command_palette::{self};
use crate::config::{FontConfig, SidebarFocus, UiFont, UiTheme};
use crate::path_style::PathStyle;
use crate::projects::{Project, Worktree};
use crate::session::{LiveState, SessionActivity, SessionId, SessionKind, TermSize};
use crate::sidebar_nav::{self, SidebarRow};
use crate::workspace::WorkspaceKey;
use crate::wsl::{self};
use crate::wsl_helper::{self, WslProbe};
use crate::{herdr, path_style};

use super::{ActionOrigin, HarnessMark, Managed, managed_tooltip};

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

/// Whether ending a session asks first.  A harness-managed one is a detach
/// rather than a kill, so it answers to its own switch: the attach client is
/// always running, which would make the busy question a close asks fire every
/// time and warn about nothing.
pub(super) fn close_needs_prompt(ui: &UiTheme, managed: bool, busy: bool) -> bool {
    if managed { ui.confirm_session_detach } else { ui.confirm_session_close.requires_prompt(busy) }
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
    use super::super::focus::DeferredClose;
    use super::super::sidebar::{HerdrRowData, RowName, herdr_display_name};
    use super::*;
    use crate::command_palette::PaletteItem;
    use crate::config::AttachMode;
    use crate::test_util::titled_herdr_agent as titled;

    const LIVE: SessionFocus = SessionFocus { scratchpad: false, exited: false };
    const EXITED: SessionFocus = SessionFocus { scratchpad: false, exited: true };
    const SCRATCHPAD: SessionFocus = SessionFocus { scratchpad: true, exited: false };

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
