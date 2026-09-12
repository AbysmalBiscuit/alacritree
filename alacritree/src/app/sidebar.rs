//! The projects sidebar: the state only it owns, the paint pass over it, and
//! the row painters that pass draws with.

use super::*;

pub(super) struct Sidebar {
    pub(super) cursor: Option<SidebarRow>,
    /// Reveals the project rows' drag grips.  A transient mode, not persisted:
    /// reordering is a rare, deliberate act, and a grip on every row the rest
    /// of the time is noise.
    pub(super) reorder_mode: bool,
    /// One-shot: scroll the cursor row into view on the next sidebar paint.
    pub(super) cursor_moved: bool,
    /// Fuzzy-search query and `s`/`a` toggle state for the projects panel.
    /// Transient: never persisted, never touches the `expanded` flag.
    pub(super) filter: PanelFilter,
    /// Rows behind the last-built focus snapshot. Paint reuses this until the
    /// next rebuild instead of recomputing the projection every frame.
    pub(super) rows_cache: Option<Vec<SidebarRow>>,
    /// The deepest row a filter hid, restored when it becomes visible again.
    pub(super) anchor: Option<SidebarRow>,
}

impl Sidebar {
    pub(super) fn new(filter: PanelFilter) -> Self {
        Self {
            cursor: None,
            reorder_mode: false,
            cursor_moved: false,
            filter,
            rows_cache: None,
            anchor: None,
        }
    }
}

impl AlacritreeApp {
    /// Tint whichever region a drop would land on while files are hovering, so
    /// three targets do not become a guessing game.  Silent off Windows: no
    /// cursor position is available there, so the tint would be a lie.
    pub(super) fn paint_drop_hover(&self, ctx: &Context, regions: &file_drop::Regions) {
        let cfg = &self.config.ui.drop;
        if !cfg.enabled || !cfg.highlight || ctx.input(|i| i.raw.hovered_files.is_empty()) {
            return;
        }
        let Some(pointer) = file_drop::screen_pointer(ctx) else {
            return;
        };
        // winit's `DragOver` handler emits no event, so moving the cursor
        // mid-drag wakes nothing and the polled position would stay frozen at
        // wherever the drag entered.  This is the only place the feature drives
        // the loop, and it stops when the drag leaves or drops.
        ctx.request_repaint();
        let active_is_scratchpad =
            self.active_session_index().is_some_and(|idx| self.sessions[idx].scratchpad.is_some());
        let Some(target) = file_drop::route(Some(pointer), regions, active_is_scratchpad, cfg)
        else {
            return;
        };
        let rect = match target {
            file_drop::Target::ProjectsSidebar => match regions.sidebar {
                Some(rect) => rect,
                None => return,
            },
            file_drop::Target::Terminal | file_drop::Target::Scratchpad => regions.central,
        };
        // `Theme::accent` is already resolved (config accent, else ANSI blue);
        // `UiTheme::sidebar_accent` is the raw `Option` and would paint nothing
        // on an unconfigured palette.
        let accent = self.theme.accent;
        ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop_hover")))
            .rect_filled(rect, 0.0, accent.linear_multiply(0.15));
    }

    /// Rows the sidebar cursor steps over this frame: the fuzzy/toggle-filtered
    /// set while a filter is active, the full visible set otherwise.
    pub(super) fn current_project_rows(&mut self) -> Vec<SidebarRow> {
        let listed = self.listed_workspace_rows();
        if !self.sidebar.filter.is_filtering() {
            return sidebar_nav::visible_rows(&self.projects, &listed);
        }

        let apply = self.sidebar.filter.toggles_apply(self.search_scope);
        let toggle_sessions = apply && self.sidebar.filter.is_toggled('s');
        let toggle_attention = apply && self.sidebar.filter.is_toggled('a');
        let pr_open = apply && self.sidebar.filter.is_toggled('o');
        let pr_draft = apply && self.sidebar.filter.is_toggled('d');
        let pr_merged = apply && self.sidebar.filter.is_toggled('m');
        let pr_closed = apply && self.sidebar.filter.is_toggled('c');
        let any_pr = pr_open || pr_draft || pr_merged || pr_closed;
        let any_toggle = any_project_toggle_active(toggle_sessions, toggle_attention, any_pr);

        // Precompute every fuzzy result before building the closures: the
        // matcher needs `&mut self.sidebar.filter`, and releasing that borrow
        // up-front lets the predicates read the rest of `&self` freely.
        let home_matches = self.sidebar.filter.matches("Home");
        let project_matches: HashMap<PathBuf, bool> = {
            let filter = &mut self.sidebar.filter;
            self.projects
                .iter()
                .map(|p| (p.root.clone(), filter.matches(p.display_name())))
                .collect()
        };
        let worktree_matches: HashMap<PathBuf, bool> = {
            let filter = &mut self.sidebar.filter;
            self.projects
                .iter()
                .flat_map(|p| p.worktrees.iter())
                .map(|wt| (wt.path.clone(), filter.matches(&wt.name)))
                .collect()
        };
        let live_branch = self
            .current_workspace
            .as_deref()
            .and_then(|p| self.git_status.get(p))
            .and_then(|c| c.current_branch());
        let current_workspace = self.current_workspace.as_deref();
        // Skipped outright while the PR dimension is inert: `worktree_pr_passes`
        // would not read the map, and building it costs a path clone per
        // worktree on a call that runs whenever the panel is filtering at all.
        let pr_matches: HashMap<PathBuf, bool> = if any_pr {
            self.projects
                .iter()
                .flat_map(|p| p.worktrees.iter())
                .map(|wt| {
                    let branch = pr_status::effective_branch(wt, current_workspace, live_branch);
                    let state = self.pr_cache.state(&wt.path, branch);
                    (
                        wt.path.clone(),
                        pr_status::pr_pass(state, pr_open, pr_draft, pr_merged, pr_closed),
                    )
                })
                .collect()
        } else {
            HashMap::new()
        };

        // Child names are resolved before the matcher borrows the filter: the
        // names come off `&self` helpers and the matcher wants `&mut
        // self.sidebar.filter`, so the two cannot be live at once.  Skipped
        // outright by `search_reaches_children` with an empty query, where
        // `matches` answers true for everything and every workspace holding
        // any child would surface, and with `[ui] search_depth` at its
        // "workspaces" default, which never descends past a workspace name.
        let child_matches: HashMap<SidebarRow, bool> =
            if search_reaches_children(self.search_depth, self.sidebar.filter.query().is_empty()) {
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
                                    session_row_name(
                                        &s.title,
                                        activity,
                                        self.session_herdr_agent(s),
                                    )
                                })
                                .map(RowName::search_text)
                                .unwrap_or_default(),
                            sidebar_nav::WorkspaceEntry::Agent(side, terminal_id) => self
                                .find_herdr_agent(side, terminal_id)
                                .map(|a| herdr_display_name(a).search_text())
                                .unwrap_or_default(),
                        };
                        (entry.row(), name)
                    })
                    .collect();
                let filter = &mut self.sidebar.filter;
                names.into_iter().map(|(row, name)| (row, filter.matches(&name))).collect()
            } else {
                HashMap::new()
            };

        let session_workspaces: Vec<WorkspaceKey> =
            self.sessions.iter().map(|s| s.working_directory.clone()).collect();
        let gate = |key: &WorkspaceKey| {
            project_toggles_pass(
                apply,
                toggle_sessions,
                sessions_filter_passes(
                    &session_workspaces,
                    &listed,
                    key,
                    self.sessions_filter_counts_detached,
                ),
                toggle_attention,
                self.workspace_needs_attention(key),
            ) && key.as_deref().is_none_or(|path| worktree_pr_passes(any_pr, &pr_matches, path))
        };
        let project_self =
            |p: &Project| !any_toggle && project_matches.get(&p.root).copied().unwrap_or(false);
        let mut name =
            |_p: &Project, wt: &Worktree| worktree_matches.get(&wt.path).copied().unwrap_or(false);
        let children_tested = !child_matches.is_empty();
        let mut child = |entry: &sidebar_nav::WorkspaceEntry| {
            child_matches.get(&entry.row()).copied().unwrap_or(false)
        };
        let child: Option<&mut dyn FnMut(&sidebar_nav::WorkspaceEntry) -> bool> =
            if children_tested { Some(&mut child) } else { None };
        sidebar_nav::filtered_rows(&self.projects, &listed, sidebar_nav::RowPredicates {
            home_gate: gate(&None),
            home_name: home_matches,
            project_self: &project_self,
            gate: &gate,
            name: &mut name,
            child,
        })
    }

    pub(super) fn show_project_sidebar(&mut self, ctx: &Context, panel_frame: Frame) -> egui::Rect {
        let view = self.project_sidebar_view(ctx);
        let theme = view.theme;
        let mut requests = SidebarRequests::default();
        let panel_resp = SidePanel::left("left_sidebar")
            .resizable(true)
            .default_width(240.0 * theme.ui_scale)
            .min_width(180.0 * theme.ui_scale)
            .frame(panel_frame)
            .show(ctx, |ui| {
                // Sidebar rows are click targets, not selectable prose; the
                // default I-beam-and-select on labels is the wrong affordance.
                ui.style_mut().interaction.selectable_labels = false;
                apply_scrollbar_style(ui, self.config.ui.scrollbar);
                ui.horizontal(|ui| {
                    panel_header_filter_ui(
                        ui,
                        "Projects",
                        &self.sidebar.filter,
                        &self.config.ui.icons.search,
                        &theme,
                        self.sidebar.filter.toggles_apply(self.search_scope),
                    );
                    projects_header_buttons(ui, &view, &mut requests);
                });
                ui.separator();

                ScrollArea::vertical().show(ui, |ui| {
                    // Inter-group spacing is emitted above the group that
                    // follows, never after the last one: trailing padding
                    // makes the content measure taller than the rows on
                    // screen, which shows a scrollbar with nothing to scroll
                    // whenever the list otherwise fits the panel.
                    let mut group_gap = 0.0_f32;
                    if !view.filtering || view.membership.home {
                        paint_home_group(
                            ui,
                            &view,
                            self.current_workspace.is_none(),
                            &mut requests,
                        );
                        group_gap = 2.0;
                    }

                    if self.projects.is_empty() {
                        ui.add_space(std::mem::take(&mut group_gap));
                        ui.label(
                            RichText::new("Click + to add a project.")
                                .color(theme.text_dim)
                                .small(),
                        );
                        ui.add_space(4.0);
                        ui.label(RichText::new("Ctrl+B to toggle").small().color(theme.text_muted));
                    } else if view.filtered_empty {
                        ui.add_space(std::mem::take(&mut group_gap));
                        ui.label(RichText::new("no matches").color(theme.text_dim).small());
                    }

                    for (idx, project) in self.projects.iter_mut().enumerate() {
                        if view.filtering && !view.membership.projects.contains(&project.root) {
                            continue;
                        }
                        ui.add_space(std::mem::take(&mut group_gap));
                        paint_project_header(ui, &view, idx, project, &mut requests);
                        if project.expanded || view.filtering {
                            paint_worktrees(
                                ui,
                                &view,
                                idx,
                                project,
                                self.current_workspace.as_deref(),
                                &self.liveness,
                                &mut requests,
                            );
                            group_gap = 4.0;
                        }
                    }
                });
            });

        self.apply_sidebar_edits(ctx, &mut requests);
        let workspace_activated = self.apply_sidebar_activations(ctx, &mut requests);
        self.poll_worktree_liveness(ctx, view.probing, &requests.drawn_worktrees);
        if self.config.ui.sidebar_click_focus {
            // A click that picks a workspace or session means "go work
            // there", so it focuses the terminal; other panel clicks focus
            // the sidebar for filter typing.  Row activations fire on the
            // release frame, after the press already focused the sidebar,
            // which is why this can't fold into the press test below.
            if workspace_activated {
                self.focus_terminal();
            } else if self.focus != PaneFocus::ProjectsSidebar
                && pressed_on_panel(ctx, &panel_resp.response)
            {
                self.focus_sidebar();
            }
        }
        panel_resp.response.rect
    }

    /// Everything the paint pass reads, gathered while `&self` helpers are
    /// still callable: the panel closure borrows `projects` mutably.
    fn project_sidebar_view(&mut self, ctx: &Context) -> SidebarView {
        // Only rows that actually paint are worth a liveness probe, and which
        // ones those are is not known until the tree, its filters and its
        // collapsed projects have all had their say.  Deciding *before* the
        // walk that this frame is not a probe frame is what keeps the other
        // ~89 frames of every 90 from collecting anything at all.
        let probing = self.config.ui.worktree_liveness
            && self.liveness_probe.is_none()
            && self.liveness.wants_probe(Instant::now());
        // The render pass cannot borrow `self.sessions`, so the dragged
        // session's own scope is resolved here: a row outside this range draws
        // no indicator and never becomes a drop.
        let drag_range: Option<(SessionId, Vec<WorkspaceKey>)> =
            egui::DragAndDrop::payload::<DraggedSession>(ctx).and_then(|dragged| {
                let (_, range) = self.reorder_range(dragged.0)?;
                Some((dragged.0, range))
            });
        let cursor_row = if self.focus == PaneFocus::ProjectsSidebar {
            self.sidebar.cursor.clone()
        } else {
            None
        };
        let cursor_moved = std::mem::take(&mut self.sidebar.cursor_moved);

        let filtering = self.sidebar.filter.is_filtering();
        let active_now = self.active_session.get(&self.current_workspace).copied();
        // egui keeps one scroll target per frame and the last writer wins, so the
        // two reasons to scroll are resolved here rather than by paint order.  An
        // explicit cursor move outranks following the terminal.
        let wants_follow = sidebar_nav::wants_follow(
            self.config.ui.sidebar_follow_active,
            cursor_moved,
            &self.last_followed,
            &self.current_workspace,
            active_now,
        );
        let rows: Vec<SidebarRow> = if filtering || wants_follow {
            match &self.sidebar.rows_cache {
                Some(rows) => rows.clone(),
                None => self.current_project_rows(),
            }
        } else {
            Vec::new()
        };
        let follow_row = wants_follow
            .then(|| {
                let project_root = sidebar_nav::project_of(&self.projects, &self.current_workspace)
                    .map(Path::to_path_buf);
                sidebar_nav::follow_scroll_row(
                    &rows,
                    &self.current_workspace,
                    active_now,
                    project_root.as_deref(),
                )
            })
            .flatten();
        if follow_row.is_some() {
            self.last_followed = (self.current_workspace.clone(), active_now);
        }

        let membership = FilterMembership::of(filtering, rows);
        let filtered_empty = filtering
            && !membership.home
            && membership.projects.is_empty()
            && membership.worktrees.is_empty();

        // Snapshot attention + agent-glyph state up-front so the `iter_mut`
        // over projects in the paint pass isn't blocked from calling back
        // into `&self` helpers.
        let mut listed = self.listed_workspace_rows();
        // The cursor can only reach a row the nav model listed, so paint keeps
        // exactly that set: a session the filter dropped would strand just
        // like an unlisted agent, so both are pruned by row membership here.
        if filtering {
            for entries in listed.values_mut() {
                entries.retain(|entry| membership.children.contains(&entry.row()));
            }
        }
        let home_rows = self.workspace_rows(&None, &listed);
        // A rendered session list carries its own per-session status; repeating
        // it on the parent row reads as noise, the same
        // rule the project row applies when expanded.  Aggregates therefore
        // apply only while the list is hidden (fewer than two sessions).
        let home_lists_sessions = WorkspaceRowData::any_session(&home_rows);
        let home_attention = !home_lists_sessions && self.workspace_needs_attention(&None);
        let home_activity = if home_lists_sessions {
            SessionActivity::Shell
        } else {
            self.workspace_activity(&None)
        };
        let projects = self.project_views(ctx, &listed);

        SidebarView {
            theme: self.theme,
            icons: self.config.ui.icons.clone(),
            probing,
            reorder_mode: self.sidebar.reorder_mode,
            session_drag: self.session_drag,
            drag_range,
            cursor_row,
            cursor_moved,
            follow_row,
            filtering,
            membership,
            filtered_empty,
            home_rows,
            home_attention,
            home_activity,
            projects,
            // Worktrees whose background removal is still running: their rows show
            // a spinner instead of the delete/new-shell controls.
            deleting_paths: self
                .modals
                .pending_deletes
                .iter()
                .map(|t| t.worktree_path.clone())
                .collect(),
            // Minimized creations, keyed by project index, rendered as spinner
            // placeholder rows until the finished worktree shows up on refresh.
            creating: self
                .modals
                .pending_creates
                .iter()
                .map(|c| (c.project_idx, c.branch.clone()))
                .collect(),
            distros: wsl::distros(),
            profile_names: self.config.profiles.iter().map(|p| p.name.clone()).collect(),
            // Name + command pairs for the worktree row's "Open session" menu.
            // The command is only ever shown as hover text, never painted.
            worktree_profiles: self
                .config
                .profiles
                .iter()
                .map(|p| (p.name.clone(), profile_command(p)))
                .collect(),
        }
    }

    /// One entry per project, aligned with `projects`, each holding one entry
    /// per worktree.
    fn project_views(
        &mut self,
        ctx: &Context,
        listed: &sidebar_nav::ListedRows,
    ) -> Vec<ProjectView> {
        let pr_enabled = self.config.ui.pr_status;
        let any_pr_toggle = any_pr_toggle_active(&self.sidebar.filter, self.search_scope);
        let current_workspace = self.current_workspace.as_deref();
        let live_branch = current_workspace
            .and_then(|p| self.git_status.get(p))
            .and_then(|cache| cache.current_branch());
        // The same path can be a worktree of two projects, and `PrCache` is
        // keyed by path alone, so a second poller would only invalidate the
        // first's lookup and burn a `gh` process every frame.
        let mut polled: HashMap<PathBuf, Option<PrInfo>> = HashMap::new();
        let mut views = Vec::with_capacity(self.projects.len());
        for project in &self.projects {
            let mut worktrees = Vec::with_capacity(project.worktrees.len());
            for wt in &project.worktrees {
                let ws = Some(wt.path.clone());
                let rows = self.workspace_rows(&ws, listed);
                // Aggregates apply only while the session list is hidden, as
                // on the home row.
                let lists_sessions = WorkspaceRowData::any_session(&rows);
                let pr = resolve_pr_info(
                    &mut polled,
                    &wt.path,
                    should_poll_pr(pr_enabled, project.expanded, any_pr_toggle),
                    || {
                        let branch =
                            pr_status::effective_branch(wt, current_workspace, live_branch);
                        self.pr_cache.poll(&wt.path, branch, ctx)
                    },
                );
                // Rendered up front: the panel closure borrows `projects` mutably, and
                // substitution over short strings is microseconds, so no cache is kept.
                // After `pr` so `$pr` sees this frame's PR number.
                worktrees.push(WorktreeView {
                    label: self.row_labels.worktree_label(wt, pr.as_ref()),
                    attention: !lists_sessions && self.workspace_needs_attention(&ws),
                    activity: if lists_sessions {
                        SessionActivity::Shell
                    } else {
                        self.workspace_activity(&ws)
                    },
                    pr,
                    rows,
                });
            }
            views.push(ProjectView {
                label: self.row_labels.project_label(project),
                attention: self.project_needs_attention(project),
                worktrees,
            });
        }
        views
    }

    /// Applies what the paint pass recorded other than activations: project
    /// edits, session drops, and the dialogs a row opens.  A pointer release
    /// clicks one widget, so at most one of these fires per frame and their
    /// order is free.
    fn apply_sidebar_edits(&mut self, ctx: &Context, requests: &mut SidebarRequests) {
        if requests.add_project {
            self.add_project_via_dialog(ctx);
        }
        if requests.reorder_toggled {
            self.sidebar.reorder_mode = !self.sidebar.reorder_mode;
        }
        if let Some(idx) = requests.refresh {
            self.refresh_project(ctx, idx);
        }
        if let Some(req) = requests.remove.take() {
            self.modals.pending_project_remove = Some(req);
        }
        if let Some((root, insert_before)) = requests.reorder.take() {
            self.move_project(&root, insert_before);
        }
        if let Some((id, workspace, position)) = requests.session_drop.take() {
            self.apply_session_drop(id, workspace, position);
        }
        if let Some((root, expanded)) = requests.expand_toggled.take() {
            state::mutate(|s| {
                if let Some(p) = s.projects.iter_mut().find(|p| p.root == root) {
                    p.expanded = expanded;
                }
            });
        }
        if let Some(root) = requests.shell_override_changed.take() {
            self.persist_project(&root);
        }
        if let Some(root) = requests.label_cleared.take() {
            self.persist_project_label(&root);
        }
        if requests.rename.is_some() {
            self.modals.pending_rename = requests.rename.take();
        }
        if let Some(path) = requests.base_picker.take() {
            self.open_base_branch_picker(path);
        }
        if let Some(path) = requests.delete.take() {
            self.request_worktree_delete(&path);
        }
        if let Some(idx) = requests.create {
            self.modals.pending_create =
                Some(CreateState::Prompt { project_idx: idx, branch: String::new(), error: None });
        }
    }

    /// Applies the clicks that act on a workspace or session, and reports
    /// whether one of them activated a workspace.
    fn apply_sidebar_activations(&mut self, ctx: &Context, requests: &mut SidebarRequests) -> bool {
        let mut workspace_activated = false;
        if requests.home {
            self.activate_home(ctx);
            workspace_activated = true;
        }
        if let Some(path) = requests.activate.take() {
            self.activate_worktree(ctx, &path);
            workspace_activated = true;
        }
        if let Some((ws, id)) = requests.activate_session.take() {
            // A stale id (session reaped this frame) self-heals next frame:
            // active_session_index() misses and adopt_active_session picks
            // an existing shell, or the empty-workspace placeholder shows.
            self.current_workspace = ws.clone();
            self.active_session.insert(ws, id);
            workspace_activated = true;
        }
        if let Some(id) = requests.close_session {
            self.request_close_session(ctx, id);
        }
        if let Some((ws, key, pane_id)) = requests.attach_herdr.take() {
            // Switches first, same as `spawn_shell` below: a refusal
            // is only visible if the workspace it happened in is on screen.
            let previous = std::mem::replace(&mut self.current_workspace, ws.clone());
            let unlisted = unlisted_pane_target(&key, &pane_id);
            if self.attach_herdr_agent(ctx, key, unlisted, ws, previous.clone(), None) {
                workspace_activated = true;
            } else {
                self.current_workspace = previous;
            }
        }
        if let Some(ws) = requests.spawn_shell.take() {
            // Spawning activates the workspace and the new session, matching
            // Ctrl+T and worktree-creation's open-on-done.  An `Err` here
            // arrived before the session record did, from a checkout git has
            // forgotten or a PTY opened inline, and hands the workspace
            // back rather than stranding the user on one with no shell, the
            // same reasoning as `activate_worktree`.  A PTY opened on a
            // worker fails after the record exists, so the switch stands and
            // `poll_pending_spawns` leaves the pane on the "no session"
            // placeholder: every workspace it could hand back to is one
            // `ensure_active_session` would spawn into and fail identically.
            let previous = std::mem::replace(&mut self.current_workspace, ws.clone());
            match self.spawn_session(ctx, ws.clone()) {
                Ok(_) => workspace_activated = true,
                Err(e) => {
                    self.current_workspace = previous;
                    self.report_spawn_failure(ctx, &ws, &e);
                },
            }
        }
        if let Some((path, name)) = requests.spawn_profile.take() {
            // Same activate-on-success and stale-row-recovery shape as
            // `spawn_shell`: a stale worktree row's `+` reaches
            // `report_spawn_failure` today, and a profile picked from the
            // same row's menu must un-grey it the same way.
            let ws = Some(path);
            let previous = std::mem::replace(&mut self.current_workspace, ws.clone());
            match self.spawn_profile_session_in(ctx, &name, ws.clone()) {
                Ok(_) => workspace_activated = true,
                Err(e) => {
                    self.current_workspace = previous;
                    self.report_spawn_failure(ctx, &ws, &e);
                },
            }
        }
        workspace_activated
    }
}

/// What the projects sidebar paints from, owned so the panel closure can
/// borrow `projects` mutably alongside it.
struct SidebarView {
    theme: Theme,
    icons: Icons,
    probing: bool,
    reorder_mode: bool,
    session_drag: bool,
    /// The dragged session and the workspaces it may land in.
    drag_range: Option<(SessionId, Vec<WorkspaceKey>)>,
    cursor_row: Option<SidebarRow>,
    cursor_moved: bool,
    follow_row: Option<SidebarRow>,
    filtering: bool,
    membership: FilterMembership,
    filtered_empty: bool,
    home_rows: Vec<WorkspaceRowData>,
    home_attention: bool,
    home_activity: SessionActivity,
    projects: Vec<ProjectView>,
    deleting_paths: HashSet<PathBuf>,
    creating: Vec<(usize, String)>,
    distros: Vec<wsl::WslDistro>,
    profile_names: Vec<String>,
    worktree_profiles: Vec<(String, String)>,
}

impl SidebarView {
    fn scrolls(&self, is_cursor: bool) -> bool {
        is_cursor && self.cursor_moved
    }

    fn follows_home(&self) -> bool {
        self.follow_row == Some(SidebarRow::Home)
    }

    fn follows_session(&self, id: SessionId) -> bool {
        self.follow_row == Some(SidebarRow::Session(id))
    }

    // `Project`/`Worktree` rows carry a `PathBuf`; matching by reference
    // here (mirroring the `cursor_row` matches) keeps every scroll
    // check on the paint path allocation-free, follow target or not.
    fn follows_project(&self, root: &Path) -> bool {
        matches!(&self.follow_row, Some(SidebarRow::Project(r)) if r.as_path() == root)
    }

    fn follows_worktree(&self, path: &Path) -> bool {
        matches!(&self.follow_row, Some(SidebarRow::Worktree(p)) if p.as_path() == path)
    }
}

/// Membership for the active filter, resolved once so paint can skip
/// non-surviving rows.  While filtering, matched projects render their
/// matched worktrees regardless of `expanded`.  That is display-only, and
/// the flag is never written.
#[derive(Default)]
struct FilterMembership {
    home: bool,
    projects: HashSet<PathBuf>,
    worktrees: HashSet<PathBuf>,
    children: HashSet<SidebarRow>,
}

impl FilterMembership {
    fn of(filtering: bool, rows: Vec<SidebarRow>) -> Self {
        let mut membership = Self { home: true, ..Self::default() };
        if filtering {
            membership.home = false;
            for row in rows {
                match row {
                    SidebarRow::Home => membership.home = true,
                    SidebarRow::Project(root) => {
                        membership.projects.insert(root);
                    },
                    SidebarRow::Worktree(path) => {
                        membership.worktrees.insert(path);
                    },
                    SidebarRow::Session(_) | SidebarRow::HerdrAgent(..) => {
                        membership.children.insert(row);
                    },
                }
            }
        }
        membership
    }
}

struct ProjectView {
    label: String,
    attention: bool,
    worktrees: Vec<WorktreeView>,
}

struct WorktreeView {
    label: String,
    pr: Option<PrInfo>,
    rows: Vec<WorkspaceRowData>,
    attention: bool,
    activity: SessionActivity,
}

/// What the paint pass asks for, applied once the panel closure has released
/// its borrows.
#[derive(Default)]
struct SidebarRequests {
    add_project: bool,
    reorder_toggled: bool,
    refresh: Option<usize>,
    remove: Option<ProjectRemoveState>,
    expand_toggled: Option<(PathBuf, bool)>,
    shell_override_changed: Option<PathBuf>,
    label_cleared: Option<PathBuf>,
    rename: Option<RenameState>,
    /// Drag-to-reorder: (dragged root, insert-before display index).
    reorder: Option<(PathBuf, usize)>,
    session_drop: Option<(SessionId, WorkspaceKey, usize)>,
    home: bool,
    activate: Option<PathBuf>,
    delete: Option<PathBuf>,
    create: Option<usize>,
    base_picker: Option<PathBuf>,
    spawn_shell: Option<WorkspaceKey>,
    spawn_profile: Option<(PathBuf, String)>,
    activate_session: Option<(WorkspaceKey, SessionId)>,
    close_session: Option<SessionId>,
    attach_herdr: Option<(WorkspaceKey, herdr::HerdrKey, String)>,
    /// Worktree rows painted on a probe frame, the only ones worth probing.
    drawn_worktrees: Vec<PathBuf>,
}

fn projects_header_buttons(ui: &mut egui::Ui, view: &SidebarView, requests: &mut SidebarRequests) {
    let theme = &view.theme;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if icon_tooltip(
            styled_icon_button(
                ui,
                &view.icons.add_project,
                DEFAULT_ADD_ICON,
                theme.text_dim,
                theme,
            ),
            "add project",
            theme.icon_tooltips,
        )
        .clicked()
        {
            requests.add_project = true;
        }
        // Lit while active: the mode is only visible as grips
        // on the rows, so the button has to say it's on.
        let (color, hint) = if view.reorder_mode {
            (theme.accent, "done reordering")
        } else {
            (theme.text_dim, "reorder projects")
        };
        if icon_tooltip(
            styled_icon_button(ui, &view.icons.reorder, DEFAULT_REORDER_ICON, color, theme),
            hint,
            theme.icon_tooltips,
        )
        .clicked()
        {
            requests.reorder_toggled = true;
        }
    });
}

/// Offers `row_rect` as a landing for the session being dragged, and records
/// the drop on release.  `slot` carries a session row's display index and id;
/// `None` is a workspace row.
fn session_drop_target(
    ui: &egui::Ui,
    view: &SidebarView,
    row_rect: egui::Rect,
    ws: &WorkspaceKey,
    slot: Option<(usize, SessionId)>,
    requests: &mut SidebarRequests,
) {
    let Some((dragged, range)) = view.drag_range.as_ref() else { return };
    // The dragged row's own edges are no-ops, so offering them
    // as targets would paint a drop that does nothing.
    if slot.is_some_and(|(_, id)| id == *dragged) {
        return;
    }
    if !range.contains(ws) {
        return;
    }
    let Some(pointer) = ui.input(|i| i.pointer.interact_pos()) else { return };
    if !row_rect.contains(pointer) {
        return;
    }
    let position = match slot {
        // A session row: the half the pointer is in decides.
        Some((idx, _)) => {
            if draw_drop_indicator(ui, row_rect, pointer, &view.theme) {
                idx
            } else {
                idx + 1
            }
        },
        // A workspace row: its sessions start under it, so a
        // drop here means the front of that workspace.  This is
        // the only way to reach a workspace listing no session
        // rows, either an empty one or a single-session one below
        // the display threshold.
        None => {
            ui.painter().hline(
                row_rect.x_range(),
                row_rect.bottom(),
                drop_indicator_stroke(&view.theme),
            );
            0
        },
    };
    if ui.input(|i| i.pointer.any_released()) {
        requests.session_drop = Some((*dragged, ws.clone(), position));
        egui::DragAndDrop::clear_payload(ui.ctx());
    }
}

fn paint_home_group(
    ui: &mut egui::Ui,
    view: &SidebarView,
    is_active: bool,
    requests: &mut SidebarRequests,
) {
    let is_cursor = matches!(&view.cursor_row, Some(SidebarRow::Home));
    let action = home_row(
        ui,
        is_active,
        is_cursor,
        view.scrolls(is_cursor) || view.follows_home(),
        view.home_attention,
        view.home_activity,
        &view.icons,
        &view.theme,
    );
    if action.activate {
        requests.home = true;
    }
    if action.spawn {
        requests.spawn_shell = Some(None);
    }
    session_drop_target(ui, view, action.rect, &None, None, requests);
    paint_workspace_children(ui, view, &view.home_rows, &None, requests);
}

/// The session and herdr rows listed under the workspace `ws`.
fn paint_workspace_children(
    ui: &mut egui::Ui,
    view: &SidebarView,
    rows: &[WorkspaceRowData],
    ws: &WorkspaceKey,
    requests: &mut SidebarRequests,
) {
    // Only rows a reorder can move take a drop slot, and
    // the slot index counts those alone: a herdr pane's
    // place is herdr's to decide, so it is neither a drag
    // subject nor a landing.
    let mut slot = 0usize;
    for row in rows {
        match row {
            WorkspaceRowData::Session(row) => {
                let is_cursor =
                    matches!(&view.cursor_row, Some(SidebarRow::Session(id)) if *id == row.id);
                let scroll = view.scrolls(is_cursor) || view.follows_session(row.id);
                let movable = row.managed.is_none();
                let act = session_row(
                    ui,
                    row,
                    is_cursor,
                    scroll,
                    view.session_drag && movable,
                    &view.icons,
                    &view.theme,
                );
                if act.activate {
                    requests.activate_session = Some((ws.clone(), row.id));
                }
                if act.close {
                    requests.close_session = Some(row.id);
                }
                if movable {
                    session_drop_target(ui, view, act.rect, ws, Some((slot, row.id)), requests);
                    slot += 1;
                }
            },
            WorkspaceRowData::Herdr(row) => {
                let is_cursor = matches!(
                    &view.cursor_row,
                    Some(SidebarRow::HerdrAgent(side, id))
                        if *side == row.side && *id == row.terminal_id
                );
                let scroll = view.scrolls(is_cursor);
                let act = herdr_row(ui, row, is_cursor, scroll, &view.icons, &view.theme);
                if act.attach {
                    requests.attach_herdr = Some((
                        ws.clone(),
                        herdr::HerdrKey {
                            side: row.side.clone(),
                            terminal_id: row.terminal_id.clone(),
                        },
                        row.pane_id.clone(),
                    ));
                }
            },
        }
    }
}

/// The project's own row: its controls, cursor, reorder drop and context menu.
fn paint_project_header(
    ui: &mut egui::Ui,
    view: &SidebarView,
    idx: usize,
    project: &mut Project,
    requests: &mut SidebarRequests,
) {
    let theme = &view.theme;
    let proj_attention = view.projects.get(idx).is_some_and(|p| p.attention);
    // Bubble attention up to the project row only when the
    // project is collapsed.  Once expanded, the actual
    // worktree rows already show the dot, and doubling it
    // on the parent reads as noise.
    let show_proj_dot = proj_attention && !project.expanded;
    // Cloned out before the row closures borrow `project`
    // mutably: the trailing closure needs them for the
    // remove-confirmation prompt.
    let project_root = project.root.clone();
    let project_name = project.display_name().to_string();
    let mut expand_clicked = false;
    let mut name_resp: Option<egui::Response> = None;
    let row_rect = row_with_trailing(
        ui,
        |ui| {
            let (clicked, resp) = project_row_title(ui, view, idx, project);
            expand_clicked = clicked;
            name_resp = Some(resp);
        },
        |ui| {
            project_row_controls(
                ui,
                view,
                idx,
                &project_root,
                &project_name,
                show_proj_dot,
                requests,
            )
        },
    );
    if expand_clicked {
        project.expanded = !project.expanded;
        requests.expand_toggled = Some((project.root.clone(), project.expanded));
    }
    let header_is_cursor =
        matches!(&view.cursor_row, Some(SidebarRow::Project(r)) if *r == project.root);
    let header_rect = egui::Rect::from_x_y_ranges(ui.max_rect().x_range(), row_rect.y_range());
    if header_is_cursor {
        paint_cursor_outline(ui, header_rect, theme);
    }
    if view.scrolls(header_is_cursor) || view.follows_project(&project.root) {
        ui.scroll_to_rect(header_rect, theme.scroll_align);
    }

    // Drop target for a reorder drag.  Detected against the
    // raw payload rather than a `dnd_drop_zone` widget so no
    // extra hover-sensing rect steals the row buttons' own
    // hover highlight.
    if let Some(dragged) = egui::DragAndDrop::payload::<DraggedProject>(ui.ctx()) {
        let pointer = ui.input(|i| i.pointer.interact_pos());
        if let Some(pointer) =
            pointer.filter(|p| row_rect.contains(*p) && dragged.0 != project.root)
        {
            let before = draw_drop_indicator(ui, row_rect, pointer, theme);
            if ui.input(|i| i.pointer.any_released()) {
                let insert_before = if before { idx } else { idx + 1 };
                requests.reorder = Some((dragged.0.clone(), insert_before));
                egui::DragAndDrop::clear_payload(ui.ctx());
            }
        }
    }

    // Right-click: rename the project, and choose which
    // shell its sessions use.
    if let Some(resp) = name_resp {
        resp.context_menu(|ui| project_context_menu(ui, view, project, requests));
    }
}

/// The project row's grip, expand arrow and name.  Reports whether the arrow
/// was clicked, with the name's response for the context menu.
fn project_row_title(
    ui: &mut egui::Ui,
    view: &SidebarView,
    idx: usize,
    project: &Project,
) -> (bool, egui::Response) {
    let theme = &view.theme;
    let icons = &view.icons;
    ui.spacing_mut().item_spacing.x = ICON_CLUSTER_SPACING;
    if view.reorder_mode {
        drag_handle(ui, theme).dnd_set_drag_payload(DraggedProject(project.root.clone()));
    }
    let (arrow_style, arrow_default, arrow_hint) = if project.expanded {
        (&icons.project_expanded, DEFAULT_PROJECT_EXPANDED_ICON, "collapse project")
    } else {
        (&icons.project_collapsed, DEFAULT_PROJECT_COLLAPSED_ICON, "expand project")
    };
    let expand_clicked = icon_tooltip(
        styled_icon_button(ui, arrow_style, arrow_default, theme.text_dim, theme),
        arrow_hint,
        theme.icon_tooltips,
    )
    .clicked();
    let name = view.projects.get(idx).map_or(project.display_name(), |p| p.label.as_str());
    let (resp, galley) = truncating_label(
        ui,
        RichText::new(name).strong().small().color(theme.text),
        theme.text,
        egui::Sense::click(),
    );
    (expand_clicked, name_tooltip(resp, name, galley.elided, theme.sidebar_tooltips))
}

/// The project row's trailing buttons: remove, refresh, new worktree, and the
/// attention dot.
fn project_row_controls(
    ui: &mut egui::Ui,
    view: &SidebarView,
    idx: usize,
    root: &Path,
    name: &str,
    show_attention: bool,
    requests: &mut SidebarRequests,
) {
    let theme = &view.theme;
    let icons = &view.icons;
    if icon_tooltip(
        styled_icon_button(ui, &icons.remove_project, DEFAULT_CLOSE_ICON, theme.text_muted, theme),
        "remove from sidebar",
        theme.icon_tooltips,
    )
    .clicked()
    {
        requests.remove =
            Some(ProjectRemoveState { root: root.to_path_buf(), name: name.to_string() });
    }
    if icon_tooltip(
        styled_icon_button(ui, &icons.refresh, DEFAULT_REFRESH_ICON, theme.text_muted, theme),
        "refresh worktrees",
        theme.icon_tooltips,
    )
    .clicked()
    {
        requests.refresh = Some(idx);
    }
    if icon_tooltip(
        styled_icon_button(ui, &icons.new_worktree, DEFAULT_ADD_ICON, theme.text_muted, theme),
        "create new worktree",
        theme.icon_tooltips,
    )
    .clicked()
    {
        requests.create = Some(idx);
    }
    if show_attention {
        icon_tooltip(attention_dot(ui, theme), ATTENTION_HINT, theme.icon_tooltips);
    }
}

fn project_context_menu(
    ui: &mut egui::Ui,
    view: &SidebarView,
    project: &mut Project,
    requests: &mut SidebarRequests,
) {
    if ui.button("Rename\u{2026}").clicked() {
        requests.rename = Some(RenameState {
            root: project.root.clone(),
            label: project.display_name().to_string(),
        });
        ui.close_menu();
    }
    if project.label.is_some() && ui.button("Reset name").clicked() {
        project.label = None;
        requests.label_cleared = Some(project.root.clone());
        ui.close_menu();
    }
    // The shell picker is hidden when there is
    // nothing to choose (no distros, no profiles)
    // so minimal setups see only the rename.
    if !view.distros.is_empty() || !view.profile_names.is_empty() {
        ui.separator();
        ui.label(RichText::new("Open in\u{2026}").color(view.theme.text_muted).small());
        shell_override_menu(ui, view, project, requests);
    }
}

/// The shell choices: automatic, the Windows shell, each WSL distro and each
/// profile, with the current override marked.
fn shell_override_menu(
    ui: &mut egui::Ui,
    view: &SidebarView,
    project: &mut Project,
    requests: &mut SidebarRequests,
) {
    let mark = |selected: bool| if selected { "• " } else { "   " };
    let auto = project.shell_override.is_none();
    if ui.button(format!("{}Auto (by location)", mark(auto))).clicked() {
        project.shell_override = None;
        requests.shell_override_changed = Some(project.root.clone());
        ui.close_menu();
    }
    let win = matches!(project.shell_override, Some(ShellChoice::Windows));
    if ui.button(format!("{}Windows shell", mark(win))).clicked() {
        project.shell_override = Some(ShellChoice::Windows);
        requests.shell_override_changed = Some(project.root.clone());
        ui.close_menu();
    }
    for distro in &view.distros {
        let selected = matches!(
            &project.shell_override,
            Some(ShellChoice::Wsl(name)) if name == &distro.name
        );
        if ui.button(format!("{}WSL ({})", mark(selected), distro.name)).clicked() {
            project.shell_override = Some(ShellChoice::Wsl(distro.name.clone()));
            requests.shell_override_changed = Some(project.root.clone());
            ui.close_menu();
        }
    }
    for name in &view.profile_names {
        let selected = matches!(
            &project.shell_override,
            Some(ShellChoice::Profile(n)) if n == name
        );
        if ui.button(format!("{}Profile: {}", mark(selected), name)).clicked() {
            project.shell_override = Some(ShellChoice::Profile(name.clone()));
            requests.shell_override_changed = Some(project.root.clone());
            ui.close_menu();
        }
    }
}

/// The worktree rows under an expanded or filter-matched project, then the
/// placeholders for its minimized creations.
fn paint_worktrees(
    ui: &mut egui::Ui,
    view: &SidebarView,
    idx: usize,
    project: &Project,
    current_workspace: Option<&Path>,
    liveness: &worktree_liveness::LivenessCache,
    requests: &mut SidebarRequests,
) {
    let states = view.projects.get(idx).map_or(&[][..], |p| p.worktrees.as_slice());
    for (wt, state) in project.worktrees.iter().zip(states) {
        if view.filtering && !view.membership.worktrees.contains(&wt.path) {
            continue;
        }
        let is_active = current_workspace == Some(&wt.path);
        paint_worktree(ui, view, wt, state, is_active, liveness.missing(&wt.path), requests);
    }
    for (_, branch) in view.creating.iter().filter(|(pi, _)| *pi == idx) {
        creating_row(ui, branch, &view.icons, &view.theme);
    }
}

fn paint_worktree(
    ui: &mut egui::Ui,
    view: &SidebarView,
    wt: &Worktree,
    state: &WorktreeView,
    is_active: bool,
    missing: Option<bool>,
    requests: &mut SidebarRequests,
) {
    let is_cursor = matches!(&view.cursor_row, Some(SidebarRow::Worktree(p)) if *p == wt.path);
    let scroll = view.scrolls(is_cursor) || view.follows_worktree(&wt.path);
    let is_deleting = view.deleting_paths.contains(&wt.path);
    // A `\\wsl.localhost\` stat boots the distro's
    // 9P server, so probing one would restart a VM
    // the user had shut down and hold it resident
    // for as long as its worktrees are listed.
    // WSL rows keep discovery's word.
    if view.probing && matches!(wsl::classify(&wt.path), wsl::Location::Windows(_)) {
        requests.drawn_worktrees.push(wt.path.clone());
    }
    let action = worktree_row(
        ui,
        wt,
        missing,
        &state.label,
        state.pr.as_ref(),
        is_active,
        is_cursor,
        scroll,
        state.attention,
        state.activity,
        is_deleting,
        &view.worktree_profiles,
        &view.icons,
        &view.theme,
    );
    if action.activate {
        requests.activate = Some(wt.path.clone());
    }
    if action.delete {
        requests.delete = Some(wt.path.clone());
    }
    if action.spawn {
        requests.spawn_shell = Some(Some(wt.path.clone()));
    }
    if action.set_base {
        requests.base_picker = Some(wt.path.clone());
    }
    if let Some(name) = action.spawn_profile {
        requests.spawn_profile = Some((wt.path.clone(), name));
    }
    let ws = Some(wt.path.clone());
    session_drop_target(ui, view, action.rect, &ws, None, requests);
    paint_workspace_children(ui, view, &state.rows, &ws, requests);
}

pub(super) struct HomeAction {
    activate: bool,
    spawn: bool,
    /// Full-width row rect, for a drop target to test the pointer against.
    rect: egui::Rect,
}

pub(super) fn home_row(
    ui: &mut egui::Ui,
    is_active: bool,
    is_cursor: bool,
    scroll_into_view: bool,
    attention: bool,
    activity: SessionActivity,
    icons: &Icons,
    theme: &Theme,
) -> HomeAction {
    // Reserve a slot *before* the labels so the hover bg paints beneath them.
    let bg_idx = ui.painter().add(egui::Shape::Noop);
    let panel_x = ui.max_rect().x_range();

    let mut spawn_clicked = false;
    let mut spawn_rect: Option<egui::Rect> = None;
    let mut hints = IconHints::default();
    // The leading and trailing groups run as sibling closures, so the status
    // slot's hint travels out separately and joins the rest afterwards.
    let mut status_hint = None;
    let frame = Frame::default().inner_margin(Margin { left: 6, right: 0, top: 3, bottom: 3 });
    let resp = frame
        .show(ui, |ui| {
            row_with_trailing(
                ui,
                |ui| {
                    status_hint = paint_row_status_icon(
                        ui,
                        theme,
                        RowStatus { attention, activity, managed: None },
                        &icons.home,
                        DEFAULT_HOME_ICON,
                        is_active,
                    );
                    ui.label(
                        RichText::new("Home")
                            .color(if is_active { theme.text } else { theme.text_dim })
                            .strong()
                            .small(),
                    );
                },
                |ui| {
                    let btn = styled_icon_button(
                        ui,
                        &icons.new_session,
                        DEFAULT_ADD_ICON,
                        theme.text_muted,
                        theme,
                    );
                    hints.add(btn.rect, "new shell");
                    spawn_rect = Some(btn.rect);
                    if btn.clicked() {
                        spawn_clicked = true;
                    }
                },
            );
        })
        .response
        .interact(egui::Sense::click());
    if let Some((rect, hint)) = status_hint {
        hints.add(rect, hint);
    }
    // The row carries no name tooltip of its own, so the icons' hints are the
    // only thing a hover here has to say.
    let resp = hints.apply(resp, theme.icon_tooltips, |resp| resp);

    // Same z-order recovery as worktree_row: the retroactive frame interact
    // shadows the inner button, so route clicks inside its rect to spawn.
    if resp.clicked() && !spawn_clicked {
        if let (Some(rect), Some(pos)) = (spawn_rect, resp.interact_pointer_pos()) {
            if rect.contains(pos) {
                spawn_clicked = true;
            }
        }
    }

    let bg = if is_active {
        theme.row_active_bg
    } else if resp.hovered() {
        theme.row_hover_bg
    } else {
        Color32::TRANSPARENT
    };
    if bg != Color32::TRANSPARENT {
        let rect = egui::Rect::from_x_y_ranges(panel_x, resp.rect.y_range());
        ui.painter().set(bg_idx, egui::Shape::rect_filled(rect, 0.0, bg));
    }
    let full_rect = egui::Rect::from_x_y_ranges(panel_x, resp.rect.y_range());
    if is_cursor {
        paint_cursor_outline(ui, full_rect, theme);
    }
    if scroll_into_view {
        ui.scroll_to_rect(full_rect, theme.scroll_align);
    }
    HomeAction { activate: resp.clicked() && !spawn_clicked, spawn: spawn_clicked, rect: full_rect }
}

pub(super) struct WorktreeAction {
    activate: bool,
    delete: bool,
    spawn: bool,
    set_base: bool,
    /// Name of the profile picked from the row's "Open session" menu, if any.
    spawn_profile: Option<String>,
    /// Full-width row rect, for a drop target to test the pointer against.
    rect: egui::Rect,
}

/// Sidebar placeholder for a worktree whose creation the user minimized: a
/// spinner stands in until `poll_pending_creates` refreshes the project and the
/// real worktree row takes its place.  Indentation and the leading glyph match
/// `worktree_row` so it lines up with its future sibling.
fn creating_row(ui: &mut egui::Ui, branch: &str, icons: &Icons, theme: &Theme) {
    let s = theme.ui_scale;
    let frame = Frame::default().inner_margin(Margin { left: 16, right: 0, top: 3, bottom: 3 });
    frame.show(ui, |ui| {
        row_with_trailing(
            ui,
            |ui| {
                let (glyph, font, color) = resolve_icon(
                    &icons.worktree,
                    DEFAULT_WORKTREE_ICON,
                    theme.text_muted,
                    10.0,
                    10.0,
                    theme,
                );
                ui.label(RichText::new(glyph).color(color).font(font));
                let (resp, galley) = truncating_label(
                    ui,
                    RichText::new(branch).color(theme.text_muted).small(),
                    theme.text_muted,
                    egui::Sense::hover(),
                );
                let _ = name_tooltip(resp, branch, galley.elided, theme.sidebar_tooltips);
            },
            |ui| {
                braille_loader(ui, 12.0 * s, theme.accent);
            },
        );
    });
}

/// Badge glyph, color, and tooltip word for a PR state.
fn pr_badge<'a>(
    icons: &'a Icons,
    theme: &Theme,
    state: PrState,
) -> (&'a IconStyle, BakedGlyph, Color32, &'static str) {
    match state {
        PrState::Open => (&icons.pr_open, DEFAULT_PR_OPEN_ICON, theme.pr_open, "open"),
        PrState::Draft => (&icons.pr_draft, DEFAULT_PR_DRAFT_ICON, theme.pr_draft, "draft"),
        PrState::Merged => (&icons.pr_merged, DEFAULT_PR_MERGED_ICON, theme.pr_merged, "merged"),
        PrState::Closed => (&icons.pr_closed, DEFAULT_PR_CLOSED_ICON, theme.pr_closed, "closed"),
    }
}

/// Badge style, color, and tooltip for an upstream state.  The tooltip names
/// the upstream ref because the glyph cannot.
pub(super) fn upstream_badge<'a>(
    icons: &'a Icons,
    theme: &Theme,
    state: &UpstreamState,
) -> (&'a IconStyle, BakedGlyph, Color32, String) {
    match state {
        UpstreamState::Level { upstream } => (
            &icons.upstream_level,
            DEFAULT_UPSTREAM_LEVEL_ICON,
            theme.upstream_level,
            format!("tracks {upstream}"),
        ),
        UpstreamState::Diverged { upstream, ahead, behind } => (
            &icons.upstream_diverged,
            DEFAULT_UPSTREAM_DIVERGED_ICON,
            theme.upstream_diverged,
            format!("tracks {upstream} — {ahead} ahead, {behind} behind"),
        ),
        UpstreamState::Gone { upstream } => (
            &icons.upstream_gone,
            DEFAULT_UPSTREAM_GONE_ICON,
            theme.upstream_gone,
            format!("{upstream} is missing locally"),
        ),
        UpstreamState::Untracked => (
            &icons.upstream_untracked,
            DEFAULT_UPSTREAM_UNTRACKED_ICON,
            theme.upstream_untracked,
            "no upstream configured".to_string(),
        ),
    }
}

/// Width the worktree row's context menu is held to, so a long profile name
/// wraps onto a second line instead of stretching the popup to fit it.
const WORKTREE_MENU_MAX_WIDTH: f32 = 220.0;

pub(super) fn worktree_row(
    ui: &mut egui::Ui,
    wt: &Worktree,
    // What the liveness probe has seen since discovery ran, if anything.
    // `Some` overrides `wt.prunable` in both directions; `None` leaves it
    // standing.  Kept out of the flag itself because that also picks between
    // `git worktree remove` and a prune, and a probe must never decide that.
    missing: Option<bool>,
    display_name: &str,
    pr: Option<&PrInfo>,
    is_active: bool,
    is_cursor: bool,
    scroll_into_view: bool,
    attention: bool,
    activity: SessionActivity,
    deleting: bool,
    // Shell profiles offered in the row's "Open session" menu: `.0` is the
    // profile name (spawned and shown as the button label), `.1` is the
    // command shown on hover.
    profiles: &[(String, String)],
    icons: &Icons,
    theme: &Theme,
) -> WorktreeAction {
    // Reserve a slot *before* the labels so the hover bg paints beneath them.
    let bg_idx = ui.painter().add(egui::Shape::Noop);
    let panel_x = ui.max_rect().x_range();

    let mut delete_clicked = false;
    let mut hints = IconHints::default();
    let mut delete_rect: Option<egui::Rect> = None;
    let mut spawn_clicked = false;
    let mut spawn_rect: Option<egui::Rect> = None;
    let mut name_elided = false;
    // The leading and trailing groups run as sibling closures, so the status
    // slot's hint travels out separately and joins the rest afterwards.
    let mut status_hint = None;
    // Discovery's word, corrected by whatever the probe has seen since.  The
    // main worktree is never offered for pruning, so it never greys either.
    let prunable = worktree_looks_gone(wt, missing);
    // right: 0 keeps the worktree `×` at the same x as the project row's `×`,
    // which has no frame margin and sits flush against the panel's outer padding.
    let frame = Frame::default().inner_margin(Margin { left: 16, right: 0, top: 3, bottom: 3 });
    let resp = frame
        .show(ui, |ui| {
            let (default_icon, default_glyph) = if wt.is_main {
                (&icons.worktree_main, DEFAULT_WORKTREE_MAIN_ICON)
            } else {
                (&icons.worktree, DEFAULT_WORKTREE_ICON)
            };
            let name_color = if prunable || deleting {
                theme.text_muted
            } else if is_active {
                theme.text
            } else {
                theme.text_dim
            };
            row_with_trailing(
                ui,
                |ui| {
                    status_hint = paint_row_status_icon(
                        ui,
                        theme,
                        RowStatus { attention, activity, managed: None },
                        default_icon,
                        default_glyph,
                        is_active,
                    );
                    let (_, galley) = truncating_label(
                        ui,
                        RichText::new(display_name).small().color(name_color),
                        name_color,
                        egui::Sense::hover(),
                    );
                    name_elided = galley.elided;
                },
                |ui| {
                    // Mid-removal the row is inert: swap its controls for a
                    // spinner so the user sees the delete is in flight.
                    if deleting {
                        braille_loader(ui, 12.0 * theme.ui_scale, theme.accent);
                        return;
                    }
                    if !wt.is_main {
                        let hover =
                            if prunable { "prune worktree" } else { "delete worktree and branch" };
                        let btn = styled_icon_button(
                            ui,
                            &icons.delete_worktree,
                            DEFAULT_CLOSE_ICON,
                            theme.text_muted,
                            theme,
                        );
                        hints.add(btn.rect, hover);
                        delete_rect = Some(btn.rect);
                        if btn.clicked() {
                            delete_clicked = true;
                        }
                    }
                    let btn = styled_icon_button(
                        ui,
                        &icons.new_session,
                        DEFAULT_ADD_ICON,
                        theme.text_muted,
                        theme,
                    );
                    hints.add(btn.rect, "new shell");
                    spawn_rect = Some(btn.rect);
                    if btn.clicked() {
                        spawn_clicked = true;
                    }
                    if let Some(info) = pr {
                        let (style, default_glyph, color, word) =
                            pr_badge(icons, theme, info.state);
                        let (glyph, font, color) =
                            resolve_icon(style, default_glyph, color, 10.0, 10.0, theme);
                        let (rect, _) = ui
                            .allocate_exact_size(row_status_icon_size(theme), egui::Sense::hover());
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            glyph,
                            font,
                            color,
                        );
                        hints.add(rect, format!("PR #{} — {word}", info.number));
                    }
                    if let Some(state) = wt.upstream.as_ref() {
                        let (style, default_glyph, color, tip) =
                            upstream_badge(icons, theme, state);
                        let (glyph, font, color) =
                            resolve_icon(style, default_glyph, color, 10.0, 10.0, theme);
                        let (rect, _) = ui
                            .allocate_exact_size(row_status_icon_size(theme), egui::Sense::hover());
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            glyph,
                            font,
                            color,
                        );
                        hints.add(rect, tip);
                    }
                },
            );
        })
        .response
        .interact(egui::Sense::click());
    if let Some((rect, hint)) = status_hint {
        hints.add(rect, hint);
    }
    let resp = hints.apply(resp, theme.icon_tooltips, |resp| {
        if prunable {
            resp.on_hover_text("worktree directory is missing — × prunes it")
        } else {
            name_tooltip(resp, display_name, name_elided, theme.sidebar_tooltips)
        }
    });

    // Frame allocates its space at end-of-show, so its retroactive `interact`
    // registers *after* the inner button in egui's z-order — meaning clicks on
    // the × land on this row response, not the button.  Recover by routing
    // clicks whose position falls inside the button rect to delete.
    if resp.clicked() && !delete_clicked && !spawn_clicked {
        if let Some(pos) = resp.interact_pointer_pos() {
            if delete_rect.is_some_and(|r| r.contains(pos)) {
                delete_clicked = true;
            } else if spawn_rect.is_some_and(|r| r.contains(pos)) {
                spawn_clicked = true;
            }
        }
    }

    let mut set_base_clicked = false;
    let mut spawn_profile_clicked: Option<String> = None;
    resp.context_menu(|ui| {
        if ui.button("Set base branch…").clicked() {
            set_base_clicked = true;
            ui.close_menu();
        }
        if !profiles.is_empty() {
            ui.separator();
            ui.label(RichText::new("Open session").color(theme.text_muted).small());
            ui.set_max_width(WORKTREE_MENU_MAX_WIDTH);
            for (i, (name, command)) in profiles.iter().enumerate() {
                let btn = ui.button(profile_menu_label(i + 1, name));
                if btn.on_hover_text(command.as_str()).clicked() {
                    spawn_profile_clicked = Some(name.clone());
                    ui.close_menu();
                }
            }
        }
    });

    let bg = if is_active {
        theme.row_active_bg
    } else if resp.hovered() {
        theme.row_hover_bg
    } else {
        Color32::TRANSPARENT
    };
    let full_rect = egui::Rect::from_x_y_ranges(panel_x, resp.rect.y_range());
    if bg != Color32::TRANSPARENT {
        ui.painter().set(bg_idx, egui::Shape::rect_filled(full_rect, 0.0, bg));
    }
    if is_cursor {
        paint_cursor_outline(ui, full_rect, theme);
    }
    if scroll_into_view {
        ui.scroll_to_rect(full_rect, theme.scroll_align);
    }
    WorktreeAction {
        // A prunable row is still worth clicking when shells are homed there;
        // `activate_worktree` turns the ones that aren't into the prune hint.
        activate: !deleting && resp.clicked() && !delete_clicked && !spawn_clicked,
        delete: delete_clicked,
        spawn: spawn_clicked,
        set_base: set_base_clicked,
        spawn_profile: spawn_profile_clicked,
        rect: full_rect,
    }
}

pub(super) struct SessionRowAction {
    activate: bool,
    close: bool,
    /// Full-width row rect, for a drop target to test the pointer against.
    rect: egui::Rect,
}

/// `draggable` makes the whole row the drag handle rather than adding a grip:
/// a session row is a tab, where a project row's own controls are what a click
/// there is usually for.
pub(super) fn session_row(
    ui: &mut egui::Ui,
    row: &SessionRowData,
    is_cursor: bool,
    scroll_into_view: bool,
    draggable: bool,
    icons: &Icons,
    theme: &Theme,
) -> SessionRowAction {
    // Reserve a slot *before* the labels so the hover bg paints beneath them.
    let bg_idx = ui.painter().add(egui::Shape::Noop);
    let panel_x = ui.max_rect().x_range();

    let mut close_clicked = false;
    let mut hints = IconHints::default();
    let mut close_rect: Option<egui::Rect> = None;
    let mut title_elided = false;
    // The leading and trailing groups run as sibling closures, so the leading
    // slots' hints travel out separately and join the rest afterwards.
    let mut status_hint = None;
    let mut managed_slot = None;
    // One indent level deeper than worktree rows (16); right: 0 keeps the ×
    // at the same x as the other rows' trailing icons.
    let frame = Frame::default().inner_margin(Margin { left: 28, right: 0, top: 3, bottom: 3 });
    let resp = frame
        .show(ui, |ui| {
            let title_color = if row.is_active { theme.text } else { theme.text_dim };
            row_with_trailing(
                ui,
                |ui| {
                    status_hint = paint_row_status_icon(
                        ui,
                        theme,
                        RowStatus {
                            attention: row.needs_attention,
                            activity: row.activity,
                            managed: row.managed.as_ref(),
                        },
                        &icons.session,
                        DEFAULT_SESSION_ICON,
                        row.is_active,
                    );
                    if let Some(managed) = &row.managed {
                        let rect = paint_managed_mark(ui, icons, theme, theme.text_muted);
                        managed_slot = Some((rect, managed_tooltip(managed)));
                    }
                    let (_, galley) = truncating_label(
                        ui,
                        row_name_text(ui, &row.name, title_color, theme.text_muted),
                        title_color,
                        egui::Sense::hover(),
                    );
                    title_elided = galley.elided;
                },
                |ui| {
                    let btn = styled_icon_button(
                        ui,
                        &icons.close_session,
                        DEFAULT_CLOSE_ICON,
                        theme.text_muted,
                        theme,
                    );
                    hints.add(btn.rect, close_button_hint(row.managed.is_some()));
                    close_rect = Some(btn.rect);
                    if btn.clicked() {
                        close_clicked = true;
                    }
                },
            );
        })
        .response
        .interact(if draggable { egui::Sense::click_and_drag() } else { egui::Sense::click() });
    if let Some((rect, hint)) = status_hint {
        hints.add(rect, hint);
    }
    if let Some((rect, hint)) = managed_slot {
        hints.add(rect, hint);
    }
    // A managed row answers with the harness's own sentence wherever the
    // pointer is not on an icon: the row the user attached is the one they
    // ask how to leave, and the name tooltip cannot say it.
    let resp = hints.apply(resp, theme.icon_tooltips, |resp| match &row.managed {
        Some(managed) if theme.icon_tooltips => resp.on_hover_text(managed_tooltip(managed)),
        _ => name_tooltip(resp, &row.name.text, title_elided, theme.sidebar_tooltips),
    });

    // Frame allocates its space at end-of-show, so its retroactive `interact`
    // registers *after* the inner button in egui's z-order — meaning clicks on
    // the × land on this row response, not the button.  Recover by routing
    // clicks whose position falls inside the button rect to close.
    if resp.clicked() && !close_clicked {
        if let (Some(rect), Some(pos)) = (close_rect, resp.interact_pointer_pos()) {
            if rect.contains(pos) {
                close_clicked = true;
            }
        }
    }

    let bg = if row.is_displayed {
        theme.row_active_bg
    } else if resp.hovered() {
        theme.row_hover_bg
    } else {
        Color32::TRANSPARENT
    };
    let full_rect = egui::Rect::from_x_y_ranges(panel_x, resp.rect.y_range());
    if bg != Color32::TRANSPARENT {
        ui.painter().set(bg_idx, egui::Shape::rect_filled(full_rect, 0.0, bg));
    }
    if is_cursor {
        paint_cursor_outline(ui, full_rect, theme);
    }
    if scroll_into_view {
        ui.scroll_to_rect(full_rect, theme.scroll_align);
    }
    if draggable {
        resp.dnd_set_drag_payload(DraggedSession(row.id));
    }
    SessionRowAction {
        activate: resp.clicked() && !close_clicked,
        close: close_clicked,
        rect: full_rect,
    }
}

/// A row's name as two spans: the context, then the identity.  Nothing
/// separates them but weight — a punctuation mark here would spell out a
/// relationship the colours already show, and the status word one used to
/// join only repeated the mark two slots to its left.
///
/// One `LayoutJob` rather than two labels, for the reasons `path_text` gives:
/// no `item_spacing` gap, no second response competing for the row's click,
/// and elision that measures the whole stream.
fn row_name_text(
    ui: &egui::Ui,
    name: &RowName,
    text_color: Color32,
    context_color: Color32,
) -> egui::WidgetText {
    let Some(context) = &name.context else {
        return RichText::new(&name.text).small().color(text_color).into();
    };
    let size = egui::TextStyle::Small.resolve(ui.style()).size;
    // A hand-built job does not inherit the ui's text valign the way RichText
    // does, so it must be carried across or the text sits off-centre against
    // the marks beside it.
    let valign = ui.text_valign();
    let mut job = egui::text::LayoutJob::default();
    for (text, color) in [(format!("{context} "), context_color), (name.text.clone(), text_color)] {
        job.append(&text, 0.0, egui::TextFormat {
            font_id: egui::FontId::new(size, egui::FontFamily::Proportional),
            color,
            valign,
            ..Default::default()
        });
    }
    job.into()
}

/// Paint the harness mark and return the rect it claimed, so the caller can
/// hang the hint on it.
fn paint_managed_mark(
    ui: &mut egui::Ui,
    icons: &Icons,
    theme: &Theme,
    color: Color32,
) -> egui::Rect {
    // 10.0 is what the status marks beside it use, and `◫` shares its em
    // height with `◇` and `●`, so the same size puts them on one optical line.
    let (glyph, font, glyph_color) =
        resolve_icon(&icons.herdr, DEFAULT_HERDR_ICON, color, 10.0, 10.0, theme);
    ui.label(RichText::new(glyph).color(glyph_color).font(font)).rect
}

/// A herdr agent nothing is attached to.  Drawn in `theme.text_dim` because
/// it is listed but not live — the same weight `worktree_gone` gives a row
/// whose checkout has been removed.  An attached agent has an ordinary
/// session row instead, so no agent is ever drawn twice.
///
/// Not draggable and carries no drop-target rect: a herdr agent has no
/// position in the session order to reorder into.
fn herdr_row(
    ui: &mut egui::Ui,
    row: &HerdrRowData,
    is_cursor: bool,
    scroll_into_view: bool,
    icons: &Icons,
    theme: &Theme,
) -> HerdrRowAction {
    // Reserve a slot *before* the label so the hover bg paints beneath it.
    let bg_idx = ui.painter().add(egui::Shape::Noop);
    let panel_x = ui.max_rect().x_range();

    let frame = Frame::default().inner_margin(Margin { left: 28, right: 0, top: 3, bottom: 3 });
    let resp = frame
        .show(ui, |ui| {
            row_with_trailing(
                ui,
                |ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(row_status_icon_size(theme), egui::Sense::hover());
                    paint_harness_mark(ui, row.managed.mark, rect, theme);
                    paint_managed_mark(ui, icons, theme, theme.text_dim);
                    let text = row_name_text(ui, &row.name, theme.text_dim, theme.text_muted);
                    let _ = truncating_label(ui, text, theme.text_dim, egui::Sense::hover());
                },
                |_ui| {},
            );
        })
        .response
        .interact(egui::Sense::click());
    let resp =
        if theme.icon_tooltips { resp.on_hover_text(managed_tooltip(&row.managed)) } else { resp };

    let bg = if resp.hovered() { theme.row_hover_bg } else { Color32::TRANSPARENT };
    let full_rect = egui::Rect::from_x_y_ranges(panel_x, resp.rect.y_range());
    if bg != Color32::TRANSPARENT {
        ui.painter().set(bg_idx, egui::Shape::rect_filled(full_rect, 0.0, bg));
    }
    if is_cursor {
        paint_cursor_outline(ui, full_rect, theme);
    }
    if scroll_into_view {
        ui.scroll_to_rect(full_rect, None);
    }
    HerdrRowAction { attach: resp.clicked() }
}
