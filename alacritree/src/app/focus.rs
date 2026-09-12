//! Sidebar cursor repair, keyboard navigation, and fuzzy-search completion.

use super::*;

pub(super) struct SidebarFocusState {
    /// `[ui] search_scope`: whether a live query stands down both panels'
    /// toggle filters.  Toggled at runtime, never persisted.
    pub(super) search_scope: SearchScope,
    /// Last reconciled snapshot, the baseline for the next cursor repair.
    pub(super) previous: Option<sidebar_focus::TreeSnapshot>,
    /// What the reconciler itself last wrote. A different value on the next
    /// pass means the user navigated through a click, session cycling, the
    /// palette, a notification, or IPC, so the anchor has been overtaken.
    pub(super) written: Option<SidebarFocusWrite>,
    /// A close verdict the reconciler still owes the terminal.
    pub(super) deferred_close: Option<DeferredClose>,
}

impl SidebarFocusState {
    pub(super) fn new(search_scope: SearchScope) -> Self {
        Self { search_scope, previous: None, written: None, deferred_close: None }
    }
}

impl AlacritreeApp {
    pub(super) fn sidebar_snapshot(
        &mut self,
        skip_worktree: Option<&Path>,
    ) -> sidebar_focus::TreeSnapshot {
        let active_workspace = self.current_workspace.as_deref();
        let active_branch = active_workspace
            .and_then(|p| self.git_panel.status.get(p))
            .and_then(|c| c.current_branch());
        let inputs = sidebar_focus::ObservedInputs::capture(
            &self.projects,
            self.session_inputs(self.observes_session_titles()),
            sidebar_focus::UiInputs {
                session_rows_always: self.session_rows_always,
                sessions_filter_counts_detached: self.sessions_filter_counts_detached,
                query: self.sidebar.filter.query(),
                toggles: self.sidebar.filter.toggle_bits(),
                toggles_apply: self
                    .sidebar
                    .filter
                    .toggles_apply(self.sidebar_focus_state.search_scope),
                pr_generation: pr_generation_for(
                    self.pr_cache.generation(),
                    any_pr_toggle_active(
                        &self.sidebar.filter,
                        self.sidebar_focus_state.search_scope,
                    ),
                ),
                active_workspace,
                active_branch,
                herdr_generation: self.herdr_generation(),
            },
        );
        let rows = self.current_project_rows();
        let live = self.session_pairs();
        let listed = self.listed_workspace_rows();
        let snapshot =
            build_sidebar_snapshot(&self.projects, &live, &listed, &rows, skip_worktree, inputs);
        // Paint reuses these until the next rebuild, so an unchanged filtering
        // frame runs no fuzzy matching at all.
        self.sidebar.rows_cache = Some(rows);
        snapshot
    }

    /// Repair the sidebar cursor against what changed since the last pass.
    /// Called twice per `update`. The first call handles input and background
    /// drains before paint. The second handles changes from
    /// `reap_exited_sessions` and paint-time clicks. A pass with nothing to
    /// do costs one `ObservedInputs` compare, which is the whole steady-state
    /// budget: there is no setting that skips this.
    pub(super) fn reconcile_sidebar_focus(&mut self, ctx: &Context) {
        if sidebar_focus_overtaken(
            &self.sidebar_focus_state.written,
            self.sidebar.cursor.as_ref(),
            &self.current_workspace,
            self.active_session.get(&self.current_workspace).copied(),
        ) {
            self.sidebar.anchor = None;
        }

        let deferred = self.sidebar_focus_state.deferred_close.take();
        let skip = deferred.as_ref().and_then(|d| d.removed_worktree.clone());

        if deferred.is_none() {
            let active_workspace = self.current_workspace.as_deref();
            let active_branch = active_workspace
                .and_then(|p| self.git_panel.status.get(p))
                .and_then(|c| c.current_branch());
            if let Some(prev) = &self.sidebar_focus_state.previous {
                let unchanged = prev.inputs.matches(
                    &self.projects,
                    self.session_inputs(self.observes_session_titles()),
                    sidebar_focus::UiInputs {
                        session_rows_always: self.session_rows_always,
                        sessions_filter_counts_detached: self.sessions_filter_counts_detached,
                        query: self.sidebar.filter.query(),
                        toggles: self.sidebar.filter.toggle_bits(),
                        toggles_apply: self
                            .sidebar
                            .filter
                            .toggles_apply(self.sidebar_focus_state.search_scope),
                        pr_generation: pr_generation_for(
                            self.pr_cache.generation(),
                            any_pr_toggle_active(
                                &self.sidebar.filter,
                                self.sidebar_focus_state.search_scope,
                            ),
                        ),
                        active_workspace,
                        active_branch,
                        herdr_generation: self.herdr_generation(),
                    },
                );
                if unchanged {
                    return;
                }
            }
        }

        let next = self.sidebar_snapshot(skip.as_deref());
        let prev = self.sidebar_focus_state.previous.take().unwrap_or_else(|| next.clone());
        let outcome = sidebar_focus::repair(
            &prev,
            &next,
            self.sidebar.cursor.as_ref(),
            self.sidebar.anchor.as_ref(),
        );

        if outcome.cursor != self.sidebar.cursor {
            self.sidebar.cursor = outcome.cursor;
            self.sidebar.cursor_moved = true;
        }
        self.sidebar.anchor = outcome.anchor;
        self.sidebar_focus_state.previous = Some(next);

        if self.config.ui.sidebar_focus.follows() {
            match (outcome.follow, deferred) {
                (Some(target), _) => self.apply_follow_target(ctx, target),
                // Nothing live to land on, so the verdict this pass took over
                // from still decides where the terminal goes.
                (None, Some(deferred)) => self.apply_close_fallback(ctx, deferred.verdict),
                (None, None) => {},
            }
        }

        self.mark_sidebar_focus_write();
    }

    /// Record the current focus triple as the reconciler's own, so the next
    /// pass does not mistake it for the user navigating.
    pub(super) fn mark_sidebar_focus_write(&mut self) {
        self.sidebar_focus_state.written = Some(SidebarFocusWrite {
            cursor: self.sidebar.cursor.clone(),
            workspace: self.current_workspace.clone(),
            active: self.active_session.get(&self.current_workspace).copied(),
        });
    }

    /// Move the terminal to a removal landing.  A workspace target adopts its
    /// active session, or its first live one when that entry went stale.
    fn apply_follow_target(&mut self, ctx: &Context, target: sidebar_focus::FollowTarget) {
        match target {
            sidebar_focus::FollowTarget::Session(id) => self.activate_session_by_id(id),
            sidebar_focus::FollowTarget::Workspace(ws) => {
                let id = self
                    .active_session
                    .get(&ws)
                    .copied()
                    .filter(|id| self.sessions.iter().any(|s| s.id == *id))
                    .or_else(|| {
                        self.sessions.iter().find(|s| s.working_directory == ws).map(|s| s.id)
                    });
                if let Some(id) = id {
                    self.activate_session_by_id(id);
                }
            },
        }
        ctx.request_repaint();
    }

    pub(super) fn apply_sidebar_nav(&mut self, ctx: &Context, key: egui::Key) {
        use egui::Key;
        let rows = self.current_project_rows();
        let cursor = match self.sidebar.cursor.clone() {
            Some(c) if rows.contains(&c) => c,
            // Stale or unseeded cursor (worktree removed, project collapsed
            // by mouse, or a filter toggle narrowing the rows out from under
            // it): land on the first row and let the next press act from
            // there. Unfiltered `rows` always leads with Home.
            _ => {
                if let Some(first) = rows.first() {
                    self.set_sidebar_cursor(first.clone());
                }
                return;
            },
        };
        match key {
            Key::ArrowUp => self.set_sidebar_cursor(sidebar_nav::step(&rows, &cursor, -1)),
            Key::ArrowDown => self.set_sidebar_cursor(sidebar_nav::step(&rows, &cursor, 1)),
            Key::ArrowRight => match &cursor {
                SidebarRow::Project(root) => {
                    let root = root.clone();
                    self.set_project_expanded(&root, true);
                },
                SidebarRow::Session(id) => {
                    let id = *id;
                    self.activate_session_by_id(id);
                    self.focus_terminal();
                },
                _ => {},
            },
            Key::ArrowLeft => match &cursor {
                SidebarRow::Project(root) => self.set_project_expanded(root, false),
                SidebarRow::Worktree(_) | SidebarRow::Session(_) | SidebarRow::HerdrAgent(..) => {
                    if let Some(target) = sidebar_nav::left_target(&rows, &cursor) {
                        self.set_sidebar_cursor(target);
                    }
                },
                SidebarRow::Home => {},
            },
            Key::Enter => self.activate_sidebar_row(ctx, &cursor),
            Key::Escape => self.focus_terminal(),
            _ => {},
        }
    }

    /// Confirm the focused sidebar's fuzzy search: leave search and land the
    /// cursor on the highlighted row, scrolled into view, keeping focus in the
    /// sidebar. Selecting a row never activates it. A subsequent browsing
    /// `Enter` activates it. No-op unless the focused panel is in search mode.
    pub(super) fn sidebar_search_confirm(&mut self) {
        match self.focus {
            PaneFocus::ProjectsSidebar
                if self.sidebar.filter.mode() == panel_filter::Mode::Search =>
            {
                let acted = self.sidebar.cursor.clone();
                if let Some(row) = acted.as_ref() {
                    self.reveal_search_row(row);
                }
                self.finish_project_search_at(acted);
            },
            PaneFocus::GitSidebar if self.git_panel.filter.mode() == panel_filter::Mode::Search => {
                let cursor = self.git_panel.cursor.clone();
                self.finish_git_search_at(cursor);
            },
            _ => {},
        }
    }

    /// Expand the project owning a worktree/session row so the row outlives the
    /// search exit: search lists matched children whatever their project's
    /// `expanded` flag says, so a child under a collapsed project would vanish
    /// the moment the query clears. Expand-only, and headers are left alone, so
    /// confirming a project row never toggles it.
    pub(super) fn reveal_search_row(&mut self, row: &SidebarRow) {
        let root = {
            let session_workspace = |id: SessionId| {
                self.sessions.iter().find(|s| s.id == id).map(|s| s.working_directory.clone())
            };
            search_reveal_root(&self.projects, session_workspace, row)
        };
        if let Some(root) = root {
            self.set_project_expanded(&root, true);
        }
    }

    /// Leave search and land the projects cursor on `requested`, falling back
    /// through `ensure_cursor` when it no longer renders. The scroll is forced
    /// rather than keyed on the cursor changing: restoring the unfiltered list
    /// can move the very same row far off-screen.
    fn finish_project_search_at(&mut self, requested: Option<SidebarRow>) {
        self.sidebar.filter.exit_search();
        let rows = self.current_project_rows();
        self.sidebar.cursor = sidebar_nav::ensure_cursor(&rows, requested.as_ref());
        self.sidebar.cursor_moved = true;
    }

    /// Git-panel counterpart of `finish_project_search_at`.
    fn finish_git_search_at(&mut self, requested: Option<git_nav::GitRow>) {
        self.git_panel.filter.exit_search();
        self.recompute_git_rows();
        self.git_panel.cursor = git_nav::ensure_cursor(&self.git_panel.rows, requested.as_ref());
        self.git_panel.cursor_moved = true;
    }

    /// Cancel the focused sidebar's fuzzy search, staying in the sidebar with the
    /// cursor on the seed row (active session / workspace, else Home). No-op
    /// unless the focused panel is in search mode.
    pub(super) fn sidebar_search_cancel(&mut self) {
        match self.focus {
            PaneFocus::ProjectsSidebar
                if self.sidebar.filter.mode() == panel_filter::Mode::Search =>
            {
                let seed = sidebar_nav::seed(
                    &self.projects,
                    self.current_workspace.as_deref(),
                    &self.listed_workspace_rows(),
                    self.active_session.get(&self.current_workspace).copied(),
                );
                self.finish_project_search_at(Some(seed));
            },
            PaneFocus::GitSidebar if self.git_panel.filter.mode() == panel_filter::Mode::Search => {
                let cursor = self.git_panel.cursor.clone();
                self.finish_git_search_at(cursor);
            },
            _ => {},
        }
    }

    /// Cancel the focused sidebar's fuzzy search and return focus to the
    /// terminal. No-op unless the focused panel is in search mode.
    pub(super) fn sidebar_search_cancel_to_terminal(&mut self) {
        match self.focus {
            PaneFocus::ProjectsSidebar
                if self.sidebar.filter.mode() == panel_filter::Mode::Search =>
            {
                self.sidebar.filter.exit_search();
                self.focus_terminal();
            },
            PaneFocus::GitSidebar if self.git_panel.filter.mode() == panel_filter::Mode::Search => {
                self.git_panel.filter.exit_search();
                self.recompute_git_rows();
                self.focus_terminal();
            },
            _ => {},
        }
    }
}
