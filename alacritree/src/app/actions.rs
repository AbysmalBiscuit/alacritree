use super::*;

impl AlacritreeApp {
    pub(super) fn dispatch_action(
        &mut self,
        ctx: &Context,
        action: BindingAction,
        origin: ActionOrigin,
    ) {
        // A palette row is dispatched with the panel still searching, and the
        // cursor operations below act on a row the query may have hidden.  The
        // keyboard path cannot reach here mid-query at all: a letter's text is
        // swallowed by the query before the binding table sees the key.
        if origin == ActionOrigin::Palette
            && matches!(&action, BindingAction::Named(n) if n.requires_project_browsing())
            && self.sidebar.filter.mode() != panel_filter::Mode::Browsing
        {
            return;
        }
        match action {
            BindingAction::Chars(bytes) => {
                if let Some(idx) = self.active_session_index() {
                    let id = self.sessions[idx].id;
                    if let Some(editor) = self.sessions[idx].scratchpad.as_mut() {
                        // Custom `Chars` bindings can carry terminal control
                        // sequences (Shift+Tab is ESC [ Z, for example).  A
                        // document should only accept actual text here; native
                        // editing keys are handled by egui's TextEdit itself.
                        if let Ok(text) = std::str::from_utf8(&bytes)
                            && !text.chars().any(|c| c.is_control() && c != '\n' && c != '\t')
                        {
                            editor.insert_at_cursor(ctx, id, text);
                        }
                    } else {
                        paste::on_terminal_input_start(&self.sessions[idx]);
                        self.sessions[idx].write(bytes);
                    }
                }
            },
            BindingAction::Named(NamedAction::Paste) => {
                self.paste_from_clipboard(ctx, Target::Clipboard);
            },
            BindingAction::Named(NamedAction::PasteSelection) => {
                self.paste_from_clipboard(ctx, Target::Primary);
            },
            BindingAction::Named(NamedAction::Copy) => {
                if let Some(idx) = self.active_session_index() {
                    if let Some(editor) = self.sessions[idx].scratchpad.as_ref() {
                        if let Some(text) = editor.selected_text(ctx, self.sessions[idx].id) {
                            clipboard::write(Target::Clipboard, &text);
                        }
                    } else {
                        paste::copy_selection(&self.sessions[idx], &self.config, Target::Clipboard);
                    }
                }
            },
            BindingAction::Named(NamedAction::CopySelection) => {
                if let Some(idx) = self.active_session_index() {
                    if let Some(editor) = self.sessions[idx].scratchpad.as_ref() {
                        if let Some(text) = editor.selected_text(ctx, self.sessions[idx].id) {
                            clipboard::write(Target::Primary, &text);
                        }
                    } else {
                        paste::copy_selection(&self.sessions[idx], &self.config, Target::Primary);
                    }
                }
            },
            BindingAction::Named(NamedAction::SpawnNewInstance) => {
                let ws = self.current_workspace.clone();
                if let Err(e) = self.spawn_session(ctx, ws.clone()) {
                    self.report_spawn_failure(ctx, &ws, &e);
                }
            },
            BindingAction::Named(NamedAction::Quit) => {
                self.modals.quit_dialog_open = true;
            },
            BindingAction::Named(NamedAction::ClearHistory) => {
                use alacritty_terminal::vte::ansi::{ClearMode, Handler};
                if let Some(idx) = self.active_session_index() {
                    if self.sessions[idx].scratchpad.is_none() {
                        self.sessions[idx].term.lock().clear_screen(ClearMode::Saved);
                    }
                }
            },
            BindingAction::Named(NamedAction::ToggleFullscreen) => {
                let on = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!on));
            },
            BindingAction::Named(NamedAction::ToggleMaximized) => {
                let on = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!on));
            },
            BindingAction::Named(NamedAction::Minimize) => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            },
            BindingAction::Named(NamedAction::SelectNextTab) => self.cycle_tabs(1),
            BindingAction::Named(NamedAction::SelectPreviousTab) => self.cycle_tabs(-1),
            BindingAction::Named(NamedAction::SelectNextSession) => self.cycle_sessions(ctx, 1),
            BindingAction::Named(NamedAction::SelectPreviousSession) => {
                self.cycle_sessions(ctx, -1);
            },
            BindingAction::Named(NamedAction::SelectTab(n)) => self.select_tab(n),
            BindingAction::Named(NamedAction::SelectLastTab) => self.select_last_tab(),
            BindingAction::Named(NamedAction::SpawnProfile(n)) => {
                match self.config.profiles.get((n - 1) as usize).map(|p| p.name.clone()) {
                    Some(name) => self.spawn_profile_session(ctx, &name),
                    None => {
                        log::warn!(
                            "SpawnProfile{n}: only {} profiles configured",
                            self.config.profiles.len()
                        );
                        self.modals.error_dialog =
                            Some(format!("SpawnProfile{n}: no such profile"));
                    },
                }
            },
            BindingAction::Named(NamedAction::NoOp) => {},
            BindingAction::Named(NamedAction::ReceiveChar) => {},
            BindingAction::Named(NamedAction::ToggleLeftSidebar) => {
                self.show_left_sidebar = !self.show_left_sidebar;
                // A deliberate visibility change opts out of the auto-shown
                // round trip, and a hidden sidebar cannot keep keyboard focus.
                self.sidebar_auto_shown = false;
                if !self.show_left_sidebar && self.focus == PaneFocus::ProjectsSidebar {
                    self.focus = PaneFocus::Terminal;
                }
                self.persist_sidebars();
            },
            BindingAction::Named(NamedAction::ToggleRightSidebar) => {
                self.show_right_sidebar = !self.show_right_sidebar;
                // A deliberate visibility change opts out of the auto-shown
                // round trip, and a hidden sidebar cannot keep keyboard focus.
                self.git_panel.auto_shown = false;
                if !self.show_right_sidebar && self.focus == PaneFocus::GitSidebar {
                    self.focus = PaneFocus::Terminal;
                }
                self.persist_sidebars();
            },
            BindingAction::Named(NamedAction::ToggleSessionRows) => {
                self.session_rows_always = !self.session_rows_always;
            },
            BindingAction::Named(NamedAction::ToggleSessionTabs) => {
                self.session_tabs_always = !self.session_tabs_always;
            },
            BindingAction::Named(NamedAction::MoveSessionUp) => self.step_session(-1),
            BindingAction::Named(NamedAction::MoveSessionDown) => self.step_session(1),
            BindingAction::Named(NamedAction::ToggleSessionDrag) => {
                self.session_drag = !self.session_drag;
            },
            BindingAction::Named(NamedAction::ToggleDetachedSessionsFilter) => {
                self.sessions_filter_counts_detached = !self.sessions_filter_counts_detached;
            },
            BindingAction::Named(NamedAction::NewMultiplexerPane) => {
                if !self.config.integrations.herdr.enabled {
                    self.modals.error_dialog = Some(HERDR_DISABLED.to_string());
                } else {
                    match self.default_multiplexer_side() {
                        Ok(side) => {
                            let workspace = self.current_workspace.clone();
                            self.create_multiplexer_pane(ctx, side, workspace, None);
                        },
                        Err(e) => self.modals.error_dialog = Some(e),
                    }
                }
            },
            BindingAction::Named(NamedAction::AttachAllMultiplexerPanes) => {
                self.attach_every_multiplexer_pane(ctx);
            },
            BindingAction::Named(NamedAction::DetachAllMultiplexerPanes) => {
                self.detach_every_multiplexer_pane(ctx);
            },
            BindingAction::Named(NamedAction::SelectNextWorkspace) => {
                self.cycle_workspaces(ctx, 1);
            },
            BindingAction::Named(NamedAction::SelectPreviousWorkspace) => {
                self.cycle_workspaces(ctx, -1);
            },
            BindingAction::Named(NamedAction::OpenScratchpad) => self.toggle_scratchpad_tab(ctx),
            BindingAction::Named(NamedAction::AddProject) => self.add_project_via_dialog(ctx),
            BindingAction::Named(NamedAction::ToggleSidebarFocus) => match self.focus {
                PaneFocus::Terminal => self.focus_sidebar(),
                PaneFocus::ProjectsSidebar => self.focus_terminal(),
                // Toggle stays "left ↔ terminal"; from the right panel it hops
                // to the left one rather than doing nothing.
                PaneFocus::GitSidebar => self.focus_sidebar(),
            },
            BindingAction::Named(NamedAction::CloseSession) => {
                let cursored = if self.focus == PaneFocus::ProjectsSidebar {
                    match &self.sidebar.cursor {
                        Some(SidebarRow::Session(id)) => Some(*id),
                        _ => None,
                    }
                } else {
                    None
                };
                let target = cursored
                    .or_else(|| self.active_session_index().map(|idx| self.sessions[idx].id));
                if let Some(id) = target {
                    self.request_close_session(ctx, id);
                }
            },
            // No confirmation and no cursor: the child is already gone, so
            // there is nothing left to interrupt and nothing to ask about.
            BindingAction::Named(NamedAction::CloseExitedSession) => {
                if let Some(idx) = self.active_session_index()
                    && self.sessions[idx].is_exited()
                {
                    let id = self.sessions[idx].id;
                    self.close_session(ctx, id);
                }
            },
            BindingAction::Named(NamedAction::SidebarTop) => self.sidebar_cursor_to_edge(true),
            BindingAction::Named(NamedAction::SidebarBottom) => self.sidebar_cursor_to_edge(false),
            BindingAction::Named(NamedAction::SidebarNextProject) => {
                self.sidebar_cursor_project_jump(1)
            },
            BindingAction::Named(NamedAction::SidebarPreviousProject) => {
                self.sidebar_cursor_project_jump(-1)
            },
            BindingAction::Named(NamedAction::RefreshProjects) => {
                self.refresh_all_projects(ctx);
            },
            BindingAction::Named(NamedAction::DeleteSelected) => {
                match self.sidebar.cursor.clone() {
                    Some(SidebarRow::Session(id)) => self.request_close_session(ctx, id),
                    Some(SidebarRow::Worktree(path)) => self.request_worktree_delete(&path),
                    Some(SidebarRow::Project(root)) => {
                        if let Some(p) = self.projects.iter().find(|p| p.root == root) {
                            self.modals.pending_project_remove = Some(ProjectRemoveState {
                                name: p.display_name().to_string(),
                                root,
                            });
                        }
                    },
                    Some(SidebarRow::Home) | Some(SidebarRow::HerdrAgent(..)) | None => {},
                }
            },
            BindingAction::Named(NamedAction::RenameSelected) => {
                // Only project rows carry an editable label; sessions and
                // worktrees take their names from the terminal title and the
                // `[ui] worktree_name` template.
                if let Some(SidebarRow::Project(root)) = self.sidebar.cursor.clone() {
                    if let Some(p) = self.projects.iter().find(|p| p.root == root) {
                        self.modals.pending_rename =
                            Some(RenameState { root, label: p.display_name().to_string() });
                    }
                }
            },
            BindingAction::Named(NamedAction::ToggleProjectExpanded) => {
                let Some(cursor) = self.sidebar.cursor.clone() else {
                    return;
                };
                let root = {
                    let session_workspace = |id: SessionId| {
                        self.sessions
                            .iter()
                            .find(|s| s.id == id)
                            .map(|s| s.working_directory.clone())
                    };
                    row_project_root(&self.projects, session_workspace, &cursor)
                };
                if let Some(root) = root {
                    let expanded =
                        self.projects.iter().find(|p| p.root == root).is_some_and(|p| p.expanded);
                    self.set_project_expanded(&root, !expanded);
                    // Collapsing hides the cursored child; move the cursor to
                    // the header so it doesn't point at a now-invisible row.
                    if expanded && !matches!(cursor, SidebarRow::Project(_)) {
                        self.set_sidebar_cursor(SidebarRow::Project(root));
                    }
                }
            },
            BindingAction::Named(NamedAction::TogglePalette) => {
                self.palette.toggle();
            },
            BindingAction::Named(NamedAction::FocusProjectsSidebar) => {
                if self.focus != PaneFocus::ProjectsSidebar {
                    self.focus_sidebar();
                }
            },
            BindingAction::Named(NamedAction::FocusGitSidebar) => {
                if self.focus != PaneFocus::GitSidebar {
                    self.focus_git_sidebar()
                } else {
                    self.focus_terminal()
                }
            },
            BindingAction::Named(NamedAction::FocusTerminal) => self.focus_terminal(),
            BindingAction::Named(NamedAction::FocusLeft) => {
                self.move_focus(FocusDir::Left, origin);
            },
            BindingAction::Named(NamedAction::FocusRight) => {
                self.move_focus(FocusDir::Right, origin);
            },
            BindingAction::Named(NamedAction::SetBaseBranch) => {
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
            BindingAction::Named(NamedAction::SidebarSearchConfirm) => {
                self.sidebar_search_confirm();
            },
            BindingAction::Named(NamedAction::SidebarSearchCancel) => {
                self.sidebar_search_cancel();
            },
            BindingAction::Named(NamedAction::SidebarSearchCancelToTerminal) => {
                self.sidebar_search_cancel_to_terminal();
            },
            BindingAction::Named(NamedAction::ClearProjectFilters) => {
                self.sidebar.filter.clear_toggles();
            },
            BindingAction::Named(NamedAction::ClearGitFilters) => {
                self.git_panel.filter.clear_toggles();
                self.after_git_filter_changed();
            },
            BindingAction::Named(NamedAction::ToggleSearchScope) => {
                self.sidebar_focus_state.search_scope = match self.sidebar_focus_state.search_scope
                {
                    SearchScope::Filtered => SearchScope::All,
                    SearchScope::All => SearchScope::Filtered,
                };
            },
            BindingAction::Named(NamedAction::RefreshPrStatus) => {
                self.pr_cache.invalidate_all();
                // The poll sites run while the sidebars paint, and the palette
                // dispatches after both have; without a wake the re-query would
                // wait for whatever repaint happened to come next.
                ctx.request_repaint();
            },
            BindingAction::Named(other) => {
                if let Some(key) = project_filter_identity(other) {
                    self.sidebar.filter.toggle(key);
                } else if let Some(key) = git_filter_identity(other) {
                    self.git_panel.filter.toggle(key);
                    self.after_git_filter_changed();
                } else {
                    self.dispatch_scroll_or_other(other);
                }
            },
            BindingAction::Unsupported(name) => {
                log::debug!("unsupported keyboard binding action: {name}");
            },
        }
    }
}
