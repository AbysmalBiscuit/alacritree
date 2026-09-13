//! The git status panel: the state only it owns, the paint pass over it, the
//! keyboard navigation and filter that drive its cursor, and the diff panes
//! its rows open.

use super::*;

/// The toggle identities the git panel accepts: modified, deleted, untracked.
pub(super) const GIT_FILTER_TOGGLES: &[char] = &['m', 'd', 'u'];

pub(super) struct GitPanel {
    /// Fuzzy-search query and `m`/`d`/`u` change-kind toggle state for the git
    /// panel.  Transient: never persisted.
    pub(super) filter: PanelFilter,
    /// Git-panel cursor, identified by `(section, path)`.  Rebuilt every render
    /// pass from `rows`, so it survives the 1.5 s status refresh.
    pub(super) cursor: Option<git_nav::GitRow>,
    /// One-shot: scroll the git cursor row into view on the next paint.
    pub(super) cursor_moved: bool,
    /// Render-order git rows the cursor steps over, refreshed by the render pass.
    pub(super) rows: Vec<git_nav::GitRow>,
    /// Resolved default-branch ref backing the git panel's branch-diff rows,
    /// refreshed by the render pass so Enter opens the same diff a click would.
    pub(super) branch_base: Option<String>,
    /// The focus toggle opened a hidden git sidebar; returning focus closes it
    /// again so a keyboard round trip leaves the layout untouched.
    pub(super) auto_shown: bool,
    pub(super) status: HashMap<PathBuf, StatusCache>,
    /// Per-worktree override of the git panel's diff base, keyed by worktree
    /// path.  Mirrors `state.toml`; written through `state::set_base_branch`.
    pub(super) base_branch_overrides: HashMap<PathBuf, String>,
    /// Resolved absolute path of `delta` inside each WSL distro, so diff panes
    /// stop re-sourcing a login profile on every open.  Successes only: a miss
    /// is never stored, so installing delta mid-session is picked up later.
    pub(super) wsl_delta_paths: HashMap<String, String>,
    /// In-flight delta discoveries, keyed by distro, mirroring
    /// `pending_project_refresh` — resolved off the UI thread, adopted in
    /// `wsl_delta_path`.
    pub(super) pending_delta: HashMap<String, jobs::Job<Option<String>>>,
}

impl GitPanel {
    pub(super) fn new(base_branch_overrides: HashMap<PathBuf, String>) -> Self {
        Self {
            status: HashMap::new(),
            filter: PanelFilter::new(GIT_FILTER_TOGGLES),
            cursor: None,
            cursor_moved: false,
            rows: Vec::new(),
            branch_base: None,
            auto_shown: false,
            base_branch_overrides,
            wsl_delta_paths: HashMap::new(),
            pending_delta: HashMap::new(),
        }
    }
}

impl AlacritreeApp {
    /// Arrow/Enter/Escape navigation while the git sidebar owns keyboard
    /// focus.  Same event-drain shape as `handle_sidebar_nav`: consumes only
    /// unmodified nav keys, leaving modifier-bound shortcuts for
    /// `handle_shortcuts`.
    pub(super) fn handle_git_sidebar_nav(&mut self, ctx: &Context) {
        let filter = &mut self.git_panel.filter;
        let bindings = &self.config.bindings;
        let steps: Vec<SidebarNavStep> = ctx.input_mut(|i| {
            let mut steps = Vec::new();
            let text_keys = keys_paired_with_text(&i.events);
            let mut idx = 0;
            i.events.retain(|ev| {
                let produced_text = text_keys[idx];
                idx += 1;
                match ev {
                    egui::Event::Text(text) => match filter.on_text(text) {
                        Some(outcome) => {
                            steps.push(SidebarNavStep::Filter(outcome));
                            false
                        },
                        None => true,
                    },
                    egui::Event::Key { key, pressed: true, modifiers, .. } => drain_search_or_nav(
                        &mut steps,
                        filter,
                        bindings,
                        *key,
                        *modifiers,
                        produced_text,
                    ),
                    _ => true,
                }
            });
            steps
        });
        for step in steps {
            match step {
                SidebarNavStep::Filter(outcome) => self.apply_git_filter_outcome(ctx, outcome),
                SidebarNavStep::Nav(key) => self.apply_git_sidebar_nav(ctx, key),
                SidebarNavStep::SearchAction(action) => {
                    self.dispatch_action(ctx, BindingAction::Named(action), ActionOrigin::Keyboard);
                },
            }
        }
    }

    fn apply_git_filter_outcome(&mut self, _ctx: &Context, outcome: panel_filter::Outcome) {
        use panel_filter::Outcome;
        match outcome {
            Outcome::FilterChanged => self.after_git_filter_changed(),
            Outcome::Consumed => {},
            Outcome::MoveCursor(delta) => self.move_git_cursor(delta),
            Outcome::LeavePanel => self.focus_terminal(),
        }
    }

    /// Repair the git cursor after the row set narrows or widens: recompute the
    /// filtered rows from the cached status so the next key event acts on them,
    /// then keep the cursor where it is when still visible, else fall to the
    /// first surviving row.
    pub(super) fn after_git_filter_changed(&mut self) {
        self.recompute_git_rows();
        let next = git_nav::ensure_cursor(&self.git_panel.rows, self.git_panel.cursor.as_ref());
        if next.as_ref() != self.git_panel.cursor.as_ref() {
            self.git_panel.cursor = next;
            self.git_panel.cursor_moved = true;
        }
    }

    fn move_git_cursor(&mut self, delta: i32) {
        let cursor = match self.git_panel.cursor.clone() {
            Some(c) if self.git_panel.rows.contains(&c) => c,
            _ => {
                if let Some(first) = self.git_panel.rows.first().cloned() {
                    self.set_git_cursor(first);
                }
                return;
            },
        };
        if let Some(row) = git_nav::step(&self.git_panel.rows, &cursor, delta) {
            self.set_git_cursor(row);
        }
    }

    /// Rebuild `rows` from the cached status under the active filter,
    /// without polling.  The render pass recomputes the same way from a fresh
    /// poll; this keeps the row set current between frames so a filter change
    /// and a following key event in the same batch agree on the rows.
    pub(super) fn recompute_git_rows(&mut self) {
        let Some(path) = self.active_session_path() else {
            self.git_panel.rows.clear();
            return;
        };
        let Some(status) = self.git_panel.status.get(&path).map(|c| c.last().clone()) else {
            self.git_panel.rows.clear();
            return;
        };
        self.git_panel.rows = self.filtered_git_rows(&status).rows;
    }

    /// Apply the git panel's kind toggles and fuzzy query to a status snapshot.
    /// With no kind toggle active every kind passes; otherwise the active
    /// toggles union (`m`: Modified/Renamed, `d`: Deleted, `u`: Untracked/Added).
    /// Conflicted rows and the branch-diff section are handled by `visible_rows`.
    fn filtered_git_rows(&mut self, status: &GitStatus) -> git_nav::GitRows {
        let apply = self.git_panel.filter.toggles_apply(self.sidebar_focus_state.search_scope);
        let m = apply && self.git_panel.filter.is_toggled('m');
        let d = apply && self.git_panel.filter.is_toggled('d');
        let u = apply && self.git_panel.filter.is_toggled('u');
        let kind_pass = move |k: ChangeKind| git_toggles_pass(m, d, u, k);
        let filter = &mut self.git_panel.filter;
        let mut query_pass = |path: &str| filter.matches(path);
        git_nav::visible_rows(
            &status.staged,
            &status.unstaged,
            &status.branch_diff,
            &kind_pass,
            &mut query_pass,
        )
    }

    fn apply_git_sidebar_nav(&mut self, ctx: &Context, key: egui::Key) {
        use egui::Key;
        let cursor = match self.git_panel.cursor.clone() {
            Some(c) if self.git_panel.rows.contains(&c) => c,
            // Stale or unseeded cursor (status refreshed the row out from under
            // it): land on the first row and let the next press act from there.
            _ => {
                if let Some(first) = self.git_panel.rows.first().cloned() {
                    self.set_git_cursor(first);
                }
                return;
            },
        };
        match key {
            Key::ArrowUp => {
                if let Some(row) = git_nav::step(&self.git_panel.rows, &cursor, -1) {
                    self.set_git_cursor(row);
                }
            },
            Key::ArrowDown => {
                if let Some(row) = git_nav::step(&self.git_panel.rows, &cursor, 1) {
                    self.set_git_cursor(row);
                }
            },
            Key::Enter => {
                if let Some(req) =
                    git_row_diff_request(&cursor, self.git_panel.branch_base.as_deref())
                {
                    self.open_diff(ctx, req);
                }
            },
            Key::Escape => self.focus_terminal(),
            _ => {},
        }
    }

    fn set_git_cursor(&mut self, row: git_nav::GitRow) {
        if self.git_panel.cursor.as_ref() != Some(&row) {
            self.git_panel.cursor = Some(row);
            self.git_panel.cursor_moved = true;
        }
    }

    fn project_default_branch_for(&self, path: &Path) -> Option<String> {
        for project in &self.projects {
            for wt in &project.worktrees {
                if wt.path == path {
                    return project.default_branch.clone();
                }
            }
        }
        None
    }

    pub(super) fn open_base_branch_picker(&mut self, worktree: PathBuf) {
        let detected = self.project_default_branch_for(&worktree);
        let job_worktree = worktree.clone();
        let job = jobs::pool().spawn(jobs::Priority::Interactive, move |blocking| {
            crate::worktree::list_branches(&job_worktree, blocking)
        });
        self.modals.pending_base_branch = Some(BaseBranchPicker {
            worktree,
            query: String::new(),
            branches: None,
            branches_job: Some(job),
            detected,
            cursor: 0,
        });
    }

    pub(super) fn apply_base_branch(&mut self, worktree: PathBuf, branch: Option<String>) {
        match &branch {
            Some(b) => {
                self.git_panel.base_branch_overrides.insert(worktree.clone(), b.clone());
            },
            None => {
                self.git_panel.base_branch_overrides.remove(&worktree);
            },
        }
        // The next `StatusCache::poll` sees the changed hint and recomputes;
        // nothing to invalidate by hand.
        state::mutate(|s| state::set_base_branch(s, &worktree, branch));
    }

    pub(super) fn show_git_sidebar(&mut self, ctx: &Context, panel_frame: Frame) -> egui::Rect {
        let theme = self.theme;
        let scrollbar = self.config.ui.scrollbar;
        let palette = self.config.palette.clone();
        let active_diff_key = self.active_diff_key();
        let diff_request: std::cell::Cell<Option<DiffRequest>> = std::cell::Cell::new(None);
        let open_picker: std::cell::Cell<Option<PathBuf>> = std::cell::Cell::new(None);
        let panel_resp = SidePanel::right("right_sidebar")
            .resizable(true)
            .default_width(300.0 * theme.ui_scale)
            .min_width(220.0 * theme.ui_scale)
            .frame(panel_frame)
            .show(ctx, |ui| {
                // Sidebar rows are click targets, not selectable prose; the
                // default I-beam-and-select on labels is the wrong affordance.
                ui.style_mut().interaction.selectable_labels = false;
                apply_scrollbar_style(ui, scrollbar);
                ui.horizontal(|ui| {
                    panel_header_filter_ui(
                        ui,
                        "Git",
                        &self.git_panel.filter,
                        &self.config.ui.icons.search,
                        &theme,
                        self.git_panel.filter.toggles_apply(self.sidebar_focus_state.search_scope),
                    );
                });
                ui.separator();

                let path = match self.active_session_path() {
                    Some(p) => p,
                    None => {
                        // No workspace, no rows: keep the cursor model from
                        // acting on stale rows left by a previous workspace.
                        self.git_panel.rows.clear();
                        self.git_panel.branch_base = None;
                        ScrollArea::vertical().show(ui, |ui| {
                            ui.label(
                                RichText::new("Open a worktree from the left sidebar.")
                                    .color(theme.text_dim)
                                    .small(),
                            );
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new("Ctrl+G to toggle").small().color(theme.text_muted),
                            );
                        });
                        return;
                    },
                };
                let workspace_home = self.workspace_home(&path);

                let project_default = self.project_default_branch_for(&path);
                let cache = self
                    .git_panel
                    .status
                    .entry(path.clone())
                    .or_insert_with(|| StatusCache::new(path.clone()));

                // Use whatever branch the cache already knows to query the PR
                // cache without waiting for a fresh compute — first frame may
                // be `None`, which `pr_cache.poll` handles by returning early.
                let cached_branch = cache.current_branch().map(str::to_string);
                let pr_info = self.pr_cache.poll(&path, cached_branch.as_deref(), ctx);
                let effective_default = effective_base_branch(
                    self.git_panel.base_branch_overrides.get(&path).map(String::as_str),
                    pr_info.as_ref().map(|p| p.base_branch.as_str()),
                    project_default.as_deref(),
                );
                // Single non-blocking poll: returns the last known status and
                // kicks off a background refresh if stale or if the hint
                // changed since the last completed compute.  Cloned so the
                // `self.git_panel.status` borrow ends before the cursor repair below
                // mutates other `self` fields.
                let status = cache.poll(effective_default.as_deref(), ctx).clone();

                // Prefer the resolved ref (e.g. `refs/remotes/origin/main`) so
                // the cursor's Enter-to-diff matches the branch section's rows.
                let git_branch_base = status
                    .default_branch_resolved
                    .clone()
                    .or_else(|| status.default_branch.clone());
                let filtering = self.git_panel.filter.is_filtering();
                let filtered = self.filtered_git_rows(&status);
                let staged_count = filtered.staged;
                let unstaged_count = filtered.unstaged;
                let branch_count = filtered.branch;
                self.git_panel.rows = filtered.rows;
                let mut staged_visible: HashSet<String> = HashSet::new();
                let mut unstaged_visible: HashSet<String> = HashSet::new();
                let mut branch_visible: HashSet<String> = HashSet::new();
                for row in &self.git_panel.rows {
                    match row.section {
                        GitSection::Staged => &mut staged_visible,
                        GitSection::Unstaged => &mut unstaged_visible,
                        GitSection::Branch => &mut branch_visible,
                    }
                    .insert(row.path.clone());
                }
                self.git_panel.branch_base = git_branch_base.clone();
                if self.focus == PaneFocus::GitSidebar {
                    let mut repaired = git_nav::ensure_cursor(
                        &self.git_panel.rows,
                        self.git_panel.cursor.as_ref(),
                    );
                    // An unseeded cursor lands on the row backing the open diff
                    // when there is one, so focusing the panel points at what
                    // the user is already looking at.
                    if self.git_panel.cursor.is_none() {
                        if let Some(active) = active_diff_key.as_deref() {
                            if let Some(row) = self.git_panel.rows.iter().find(|r| {
                                git_row_diff_request(r, git_branch_base.as_deref())
                                    .is_some_and(|req| diff_key(&req) == active)
                            }) {
                                repaired = Some(row.clone());
                            }
                        }
                    }
                    self.git_panel.cursor = repaired;
                }
                let cursor_row = if self.focus == PaneFocus::GitSidebar {
                    self.git_panel.cursor.clone()
                } else {
                    None
                };
                let cursor_moved = std::mem::take(&mut self.git_panel.cursor_moved);

                ScrollArea::vertical().show(ui, |ui| {
                    if let Some(err) = &status.error {
                        ui.label(
                            RichText::new(err).color(rgb_to_color32(palette.normal[1])).small(),
                        );
                        return;
                    }

                    path_header_label(
                        ui,
                        &wsl::display_path(&path),
                        theme.text_muted,
                        &theme,
                        theme.path_style.git_header,
                        workspace_home.as_deref(),
                    );
                    if let Some(branch) = &status.branch {
                        // A greedy `truncate()` label in a plain `horizontal` row
                        // consumes all the width, shoving any trailing widgets past
                        // the panel edge. Since the right sidebar's `ScrollArea`
                        // grows to fit its content, that overflow ratchets the whole
                        // panel wider every frame until the full branch name fits.
                        // Pin `vs <default>` to the right and let the current branch
                        // truncate in the space that's left, so the row can't overflow.
                        let default = status
                            .default_branch
                            .as_deref()
                            .filter(|default| *default != branch.as_str());
                        row_with_trailing(
                            ui,
                            |ui| {
                                ui.label(RichText::new("on").color(theme.text_muted).small());
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(branch).color(theme.accent).small().strong(),
                                    )
                                    .truncate(),
                                );
                            },
                            |ui| {
                                if let Some(default) = default {
                                    // right_to_left: default sits rightmost, `vs` to its left.
                                    let resp = icon_tooltip(
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(default)
                                                    .color(theme.text_dim)
                                                    .small(),
                                            )
                                            .truncate()
                                            .sense(egui::Sense::click()),
                                        )
                                        .on_hover_cursor(egui::CursorIcon::PointingHand),
                                        "Set the branch this panel diffs against",
                                        theme.icon_tooltips,
                                    );
                                    if resp.clicked() {
                                        open_picker.set(Some(path.clone()));
                                    }
                                    ui.label(RichText::new("vs").color(theme.text_muted).small());
                                }
                            },
                        );
                    }
                    let mut section_gap = 10.0_f32;

                    section(
                        ui,
                        &theme,
                        "Staged",
                        staged_count,
                        filtering,
                        &mut section_gap,
                        |ui| {
                            for f in &status.staged {
                                if !staged_visible.contains(&f.path) {
                                    continue;
                                }
                                let req = DiffRequest {
                                    file: f.path.clone(),
                                    source: DiffSource::Staged,
                                };
                                let is_active = active_diff_key.as_deref() == Some(&diff_key(&req));
                                let resp = file_row(ui, f, &theme, &palette, is_active);
                                if resp.clicked() {
                                    diff_request.set(Some(req));
                                }
                                paint_git_row_cursor(
                                    ui,
                                    &resp,
                                    &cursor_row,
                                    GitSection::Staged,
                                    &f.path,
                                    cursor_moved,
                                    &theme,
                                );
                            }
                        },
                    );

                    section(
                        ui,
                        &theme,
                        "Unstaged",
                        unstaged_count,
                        filtering,
                        &mut section_gap,
                        |ui| {
                            for f in &status.unstaged {
                                if !unstaged_visible.contains(&f.path) {
                                    continue;
                                }
                                let source = if f.kind == ChangeKind::Untracked {
                                    DiffSource::Untracked
                                } else {
                                    DiffSource::Worktree
                                };
                                let req = DiffRequest { file: f.path.clone(), source };
                                let is_active = active_diff_key.as_deref() == Some(&diff_key(&req));
                                let resp = file_row(ui, f, &theme, &palette, is_active);
                                if resp.clicked() {
                                    diff_request.set(Some(req));
                                }
                                paint_git_row_cursor(
                                    ui,
                                    &resp,
                                    &cursor_row,
                                    GitSection::Unstaged,
                                    &f.path,
                                    cursor_moved,
                                    &theme,
                                );
                            }
                        },
                    );

                    if !status.branch_diff.is_empty() {
                        let base_label = match &status.default_branch {
                            Some(b) => format!("Changes vs {b}"),
                            None => "Changes vs default".to_string(),
                        };
                        let base = git_branch_base.clone();
                        let count_label = section_count_label(&branch_count, filtering);

                        ui.add_space(std::mem::take(&mut section_gap));
                        // Open-coded section header so the PR number can be a
                        // hyperlink while the rest stays plain text.
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&base_label).color(theme.text).strong().small());
                            if let Some(pr) = &pr_info {
                                ui.label(RichText::new("·").color(theme.text_muted).small());
                                ui.hyperlink_to(
                                    RichText::new(format!("PR #{}", pr.number))
                                        .color(theme.accent)
                                        .small()
                                        .strong(),
                                    &pr.url,
                                );
                            }
                            ui.label(RichText::new(count_label).color(theme.text_muted).small());
                        });
                        ui.add_space(2.0);
                        for stat in &status.branch_diff {
                            if !branch_visible.contains(&stat.path) {
                                continue;
                            }
                            let Some(base) = base.clone() else {
                                let resp = branch_diff_row(ui, stat, &theme, &palette, false);
                                paint_git_row_cursor(
                                    ui,
                                    &resp,
                                    &cursor_row,
                                    GitSection::Branch,
                                    &stat.path,
                                    cursor_moved,
                                    &theme,
                                );
                                continue;
                            };
                            let req = DiffRequest {
                                file: stat.path.clone(),
                                source: DiffSource::Branch { base },
                            };
                            let is_active = active_diff_key.as_deref() == Some(&diff_key(&req));
                            let resp = branch_diff_row(ui, stat, &theme, &palette, is_active);
                            if resp.clicked() {
                                diff_request.set(Some(req));
                            }
                            paint_git_row_cursor(
                                ui,
                                &resp,
                                &cursor_row,
                                GitSection::Branch,
                                &stat.path,
                                cursor_moved,
                                &theme,
                            );
                        }
                    }
                });
            });
        if let Some(req) = diff_request.take() {
            self.open_diff(ctx, req);
        }
        if let Some(path) = open_picker.take() {
            self.open_base_branch_picker(path);
        }
        if self.config.ui.sidebar_click_focus
            && self.focus != PaneFocus::GitSidebar
            && pressed_on_panel(ctx, &panel_resp.response)
        {
            self.focus_git_sidebar();
        }
        panel_resp.response.rect
    }

    /// Clicking a sidebar row either opens, replaces, or closes the workspace's
    /// single diff pane:
    /// - row matches the active diff → toggle off (close)
    /// - row matches a different diff → drop the old pane, open this one
    /// - no active diff → open a new pane
    /// Dropping the old `Session` runs `Drop`, which sends `Msg::Shutdown` to
    /// the event loop and exits delta cleanly.
    fn open_diff(&mut self, ctx: &Context, req: DiffRequest) {
        let Some(workspace) = self.current_workspace.clone() else {
            return;
        };
        let new_key = diff_key(&req);
        let existing = self.sessions.iter().find(|s| {
            s.working_directory.as_deref() == Some(&workspace)
                && matches!(&s.kind, SessionKind::Diff { .. })
        });
        if let Some(session) = existing {
            let id = session.id;
            if matches!(&session.kind, SessionKind::Diff { key } if key == &new_key) {
                // Routing through close_session applies the same
                // sibling-promotion and fallback navigation as any other
                // close, so toggling off the diff pane never strands the
                // workspace on an empty view.
                self.close_session(ctx, id);
                return;
            }
            self.sessions.retain(|s| s.id != id);
        }

        let delta_override = self.config.delta_path.clone();
        let (program, args) = match wsl::classify(&workspace) {
            wsl::Location::Wsl { distro, .. } => match delta_override {
                Some(delta) => build_wsl_diff_command_direct(&distro, &workspace, &req, &delta),
                None => match self.wsl_delta_path(&distro, ctx) {
                    Some(delta) => build_wsl_diff_command_direct(&distro, &workspace, &req, &delta),
                    None => build_wsl_diff_command_login(&distro, &workspace, &req),
                },
            },
            wsl::Location::Windows(_) => {
                build_diff_command(delta_override.as_deref().unwrap_or("delta"), &req)
            },
        };
        let title = format!(
            "diff: {}",
            path_style::render(&req.file, self.config.ui.path_style.diff_title, None)
        );
        let (size, cell_size) = self.next_spawn_geometry();
        let (session, request) = Session::pending_command(
            ctx.clone(),
            &self.config,
            Some(workspace.clone()),
            size,
            cell_size,
            program,
            args,
            title,
            SessionKind::Diff { key: new_key },
        );
        match self.open_session(session, request) {
            Ok(id) => {
                self.active_session.insert(Some(workspace), id);
            },
            Err(e) => {
                self.modals.error_dialog = Some(format!("failed to open diff: {e}"));
            },
        }
    }

    /// Cached absolute path of `delta` inside `distro`, if known.  Adopts a
    /// finished background discovery, then spawns one when the path is neither
    /// cached nor already in flight.  Returns `None` until the first discovery
    /// lands — callers fall back to the login-shell command meanwhile.  A miss
    /// is never cached, so the discovery re-runs and a mid-session install is
    /// picked up on a later open.
    fn wsl_delta_path(&mut self, distro: &str, ctx: &Context) -> Option<String> {
        match self.git_panel.pending_delta.get(distro).map(|job| (job.poll(), job.failed())) {
            Some((Some(Some(path)), _)) => {
                self.git_panel.pending_delta.remove(distro);
                self.git_panel.wsl_delta_paths.insert(distro.to_string(), path);
            },
            // A found-nothing landing and a panicked lookup both clear the
            // pending entry: the former banked its answer, the latter has
            // none to bank, and either way it must not wedge this distro out
            // of ever being retried.
            Some((Some(None), _)) | Some((None, true)) => {
                self.git_panel.pending_delta.remove(distro);
            },
            _ => {},
        }

        if let Some(path) = self.git_panel.wsl_delta_paths.get(distro) {
            return Some(path.clone());
        }

        if !self.git_panel.pending_delta.contains_key(distro) {
            let distro_owned = distro.to_string();
            let ctx = ctx.clone();
            let job = jobs::pool().spawn(jobs::Priority::Background, move |blocking| {
                let found = wsl::discover_delta(&distro_owned, blocking);
                ctx.request_repaint();
                found
            });
            self.git_panel.pending_delta.insert(distro.to_string(), job);
        }
        None
    }

    /// Key of the diff currently displayed in this workspace, if any.  Used by
    /// the sidebar to highlight the originating row so the toggle-on-reclick
    /// behavior is discoverable.
    fn active_diff_key(&self) -> Option<String> {
        self.sessions.iter().find_map(|s| {
            if s.working_directory != self.current_workspace {
                return None;
            }
            if let SessionKind::Diff { key } = &s.kind { Some(key.clone()) } else { None }
        })
    }
}

/// Render a collapsed-when-empty git section.
///
/// Empty sections are skipped entirely — a placeholder glyph for "no files
/// here" added visual noise without communicating anything the count badge
/// didn't already say.
///
/// `gap` carries the inter-section spacing: consumed above a section that
/// renders and re-armed below it, so spacing lands between sections but never
/// after the last one — trailing padding would make the content overflow the
/// panel and show a scrollbar with nothing to scroll.
fn section<R>(
    ui: &mut egui::Ui,
    theme: &Theme,
    title: &str,
    count: SectionCount,
    filtering: bool,
    gap: &mut f32,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) {
    if count.total == 0 {
        return;
    }
    ui.add_space(std::mem::take(gap));
    let label = section_count_label(&count, filtering);
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).color(theme.text).strong().small());
        ui.label(RichText::new(label).color(theme.text_muted).small());
    });
    ui.add_space(2.0);
    add_contents(ui);
    *gap = 10.0;
}

pub(super) fn file_row(
    ui: &mut egui::Ui,
    change: &FileChange,
    theme: &Theme,
    palette: &crate::config::Palette,
    is_active: bool,
) -> egui::Response {
    let bg_idx = ui.painter().add(egui::Shape::Noop);
    let panel_x = ui.max_rect().x_range();
    let row_h = ui.spacing().interact_size.y;
    let color = match change.kind {
        ChangeKind::Added | ChangeKind::Untracked => rgb_to_color32(palette.normal[2]),
        ChangeKind::Modified => rgb_to_color32(palette.normal[3]),
        ChangeKind::Deleted => rgb_to_color32(palette.normal[1]),
        ChangeKind::Renamed => rgb_to_color32(palette.normal[4]),
        ChangeKind::Conflicted => rgb_to_color32(palette.bright[1]),
    };
    let path_color = if is_active { theme.text } else { theme.text_dim };
    let mut path_galley = None;
    let mut hints = IconHints::default();
    // `ui.horizontal` sizes its response rect to the (often short) path text,
    // leaving most of the row's width as a dead zone — and short labels make
    // the row barely taller than the text, so vertical misses are easy too.
    // Allocate an explicit interact-sized row and pad it out so the click hit
    // box spans the full panel width and the row's full height.
    let resp = ui
        .allocate_ui_with_layout(
            egui::vec2(ui.available_width(), row_h),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_min_height(row_h);
                // Labels default to `Sense::click_and_drag` for text selection;
                // hit testing picks the smallest covering widget, so a clickable
                // label inside our row would eat clicks before the row sees
                // them.  Opt out of selection on every label that lives inside
                // a clickable row so the click falls through.
                let badge = ui.add(
                    egui::Label::new(
                        RichText::new(change.kind.glyph()).color(color).monospace().small(),
                    )
                    .selectable(false),
                );
                hints.add(badge.rect, change.kind.label());
                let (_, galley) = git_path_label(ui, &change.path, path_color, theme);
                path_galley = Some(galley);
                fill_row(ui);
            },
        )
        .response
        .interact(egui::Sense::click());
    let resp = hints.apply(resp, theme.icon_tooltips, |resp| {
        git_path_tooltip(resp, path_galley.as_deref(), theme)
    });
    paint_row_bg(ui, &resp, bg_idx, panel_x, theme, is_active);
    resp
}

pub(super) fn branch_diff_row(
    ui: &mut egui::Ui,
    stat: &crate::git_status::DiffStat,
    theme: &Theme,
    palette: &crate::config::Palette,
    is_active: bool,
) -> egui::Response {
    let bg_idx = ui.painter().add(egui::Shape::Noop);
    let panel_x = ui.max_rect().x_range();
    let row_h = ui.spacing().interact_size.y;
    let added = rgb_to_color32(palette.normal[2]);
    let removed = rgb_to_color32(palette.normal[1]);
    let path_color = if is_active { theme.text } else { theme.text_dim };
    let mut path_galley = None;

    // Same shape as row_with_trailing (right_to_left wrapping a left_to_right)
    // so +/- counts pin to the right edge while the path truncates cleanly;
    // `set_min_height` + `fill_row` push the hit box to the full row size.
    let resp = ui
        .allocate_ui_with_layout(
            egui::vec2(ui.available_width(), row_h),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                ui.set_min_height(row_h);
                if stat.deletions > 0 {
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("-{}", stat.deletions))
                                .color(removed)
                                .small()
                                .monospace(),
                        )
                        .selectable(false),
                    );
                }
                if stat.additions > 0 {
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("+{}", stat.additions))
                                .color(added)
                                .small()
                                .monospace(),
                        )
                        .selectable(false),
                    );
                }
                let remaining = ui.available_width();
                if remaining > 0.0 {
                    ui.allocate_ui_with_layout(
                        egui::vec2(remaining, row_h),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.set_min_height(row_h);
                            let (_, galley) = git_path_label(ui, &stat.path, path_color, theme);
                            path_galley = Some(galley);
                            fill_row(ui);
                        },
                    );
                }
            },
        )
        .response
        .interact(egui::Sense::click());
    let resp = git_path_tooltip(resp, path_galley.as_deref(), theme);
    paint_row_bg(ui, &resp, bg_idx, panel_x, theme, is_active);
    resp
}

/// The git panel's header path.  It stays selectable although the panel turns
/// label selection off, and — being a header rather than a row — keeps
/// `egui::Label`'s own elided-text tooltip instead of answering to
/// `[ui] sidebar_tooltips`.
pub(super) fn path_header_label(
    ui: &mut egui::Ui,
    path: &str,
    base: Color32,
    theme: &Theme,
    style: PathStyle,
    home: Option<&str>,
) -> egui::Response {
    let text = path_text(ui, path, base, theme, style, egui::FontFamily::Proportional, home);
    ui.add(egui::Label::new(text).truncate().selectable(true))
}

/// A git panel row's path, laid out rather than added as an `egui::Label` so
/// its tooltip is the row's to give: the label covers only the text, and a
/// pointer sweeping down the panel spends most of its time past the end of
/// short paths, where a label-borne tooltip would go quiet.  The row passes the
/// galley back through `git_path_tooltip` once it has its full-width response.
pub(super) fn git_path_label(
    ui: &mut egui::Ui,
    path: &str,
    base: Color32,
    theme: &Theme,
) -> (egui::Response, Arc<egui::Galley>) {
    let text = path_text(
        ui,
        path,
        base,
        theme,
        theme.path_style.git_rows,
        egui::FontFamily::Proportional,
        None,
    );
    truncating_label(ui, text, base, egui::Sense::hover())
}

/// Offer the row's own response the path its label painted, once the row has
/// one to hang it off.
fn git_path_tooltip(
    resp: egui::Response,
    galley: Option<&egui::Galley>,
    theme: &Theme,
) -> egui::Response {
    match galley {
        Some(galley) => name_tooltip(resp, galley.text(), galley.elided, theme.sidebar_tooltips),
        None => resp,
    }
}

/// Outline the git row the keyboard cursor rests on, matched by section+path so
/// it survives the status refresh.  Full-width rect from the panel plus the
/// row's `y_range`, mirroring the project rows.
fn paint_git_row_cursor(
    ui: &egui::Ui,
    resp: &egui::Response,
    cursor: &Option<git_nav::GitRow>,
    section: GitSection,
    path: &str,
    scroll_into_view: bool,
    theme: &Theme,
) {
    if !matches!(cursor, Some(c) if c.section == section && c.path == path) {
        return;
    }
    let rect = egui::Rect::from_x_y_ranges(ui.max_rect().x_range(), resp.rect.y_range());
    paint_cursor_outline(ui, rect, theme);
    if scroll_into_view {
        ui.scroll_to_rect(rect, theme.scroll_align);
    }
}

impl AlacritreeApp {
    pub(super) fn dispatch_git_action(&mut self, ctx: &Context, action: NamedAction) -> bool {
        match action {
            NamedAction::SetBaseBranch => {
                let target = base_branch_target(
                    self.focus == PaneFocus::ProjectsSidebar,
                    self.sidebar.cursor.as_ref(),
                    |id| {
                        self.sessions
                            .iter()
                            .find(|s| s.id == id)
                            .map(|s| s.working_directory.clone())
                    },
                    &self.current_workspace,
                );
                if let Some(path) = target {
                    self.open_base_branch_picker(path);
                }
            },
            NamedAction::ClearGitFilters => {
                self.git_panel.filter.clear_toggles();
                self.after_git_filter_changed();
            },
            NamedAction::ToggleRightSidebar => {
                self.show_right_sidebar = !self.show_right_sidebar;
                // A deliberate visibility change opts out of the auto-shown
                // round trip, and a hidden sidebar cannot keep keyboard focus.
                self.git_panel.auto_shown = false;
                if !self.show_right_sidebar && self.focus == PaneFocus::GitSidebar {
                    self.focus = PaneFocus::Terminal;
                }
                self.persist_sidebars();
            },
            NamedAction::FocusGitSidebar => {
                if self.focus != PaneFocus::GitSidebar {
                    self.focus_git_sidebar()
                } else {
                    self.focus_terminal()
                }
            },
            NamedAction::RefreshPrStatus => {
                self.pr_cache.invalidate_all();
                // The poll sites run while the sidebars paint, and the palette
                // dispatches after both have; without a wake the re-query would
                // wait for whatever repaint happened to come next.
                ctx.request_repaint();
            },
            _ => return false,
        }
        true
    }

    pub(super) fn dispatch_git_filter(&mut self, action: NamedAction) -> bool {
        let Some(key) = git_filter_identity(action) else { return false };
        self.git_panel.filter.toggle(key);
        self.after_git_filter_changed();
        true
    }
}
