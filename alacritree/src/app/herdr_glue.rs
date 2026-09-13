use super::*;

pub(super) struct HerdrGlue {
    /// Shared-view attaches whose herdr calls are running on the pool,
    /// adopted in `poll_herdr_attach`.
    pub(super) pending_attach: Vec<PendingHerdrAttach>,
    /// Pane creates running on the pool, adopted in `poll_herdr_create`.
    pub(super) pending_create: Vec<PendingHerdrCreate>,
    /// The shared view herdr was last focused for, and the call still on its
    /// way, both owned by `sync_herdr_view_focus`.
    pub(super) focused_view: herdr::HerdrViewSync,
    pub(super) view_focus: Option<herdr::HerdrViewFocus>,
    /// The herdr servers this app talks to: the native side plus one per
    /// running WSL distro, kept in step with which distros are up.
    pub(super) endpoints: herdr::Endpoints,
}

impl HerdrGlue {
    pub(super) fn new() -> Self {
        Self {
            pending_attach: Vec::new(),
            pending_create: Vec::new(),
            focused_view: herdr::HerdrViewSync::default(),
            view_focus: None,
            endpoints: herdr::Endpoints::default(),
        }
    }

    pub(super) fn close_session(&mut self, id: SessionId, herdr_key: Option<&herdr::HerdrKey>) {
        if let Some(key) = herdr_key {
            // A plain `retain` would drop a queued attach's waiters with it,
            // leaving a parked client to time out rather than learn why.
            let (removed, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending_attach)
                .into_iter()
                .partition(|pending| &pending.key == key);
            self.pending_attach = kept;
            for pending in removed {
                for waiter in pending.waiters {
                    let _ = waiter.send(Err("the session behind this pane was closed before the \
                                             attach finished"
                        .to_string()));
                }
            }
        }
        if self.view_focus.as_ref().is_some_and(|pending| pending.session == id) {
            self.view_focus = None;
        }
        self.focused_view.closed(id, herdr_key, Instant::now());
    }
}

impl AlacritreeApp {
    /// Opens a herdr agent in a session running herdr's attach client.  The
    /// session is an ordinary shell, so nothing in the grid or input path
    /// treats it specially; only the key marks it as this agent's row.
    /// Returns whether the attach succeeded so the caller can switch
    /// `current_workspace` to `workspace` first and restore it on failure —
    /// the same replace-and-restore shape `spawn_shell_request` uses, needed
    /// here for the same reason: a refusal is only readable in the
    /// workspace it happened in.  `unlisted` stands in for a pane the
    /// listing does not carry.
    pub(super) fn attach_herdr_agent(
        &mut self,
        ctx: &Context,
        key: herdr::HerdrKey,
        unlisted: PaneTarget,
        workspace: WorkspaceKey,
        previous: WorkspaceKey,
        waiter: Option<mpsc::Sender<ipc::IpcResult>>,
    ) -> bool {
        if let Some(id) = self.herdr_session_for(&key) {
            self.activate_session_by_id(id);
            if let Some(waiter) = waiter {
                let _ = waiter.send(Ok(json!({ "session_id": id })));
            }
            return true;
        }
        let target = self
            .find_herdr_agent(&key.side, &key.terminal_id)
            .map_or(unlisted, |agent| agent.target(&key.side));
        let multiplexer = Multiplexer::owning(&key);
        let attach = self.config.integrations.herdr.attach;
        if let Some(launch) = multiplexer.open_multiplexer_session(&target, attach) {
            // Nothing to ask herdr first: the pane id is the whole target,
            // and the client attaches to it directly.
            let opened =
                self.open_herdr_session(ctx, key, workspace, launch.program, launch.argv, false);
            return match opened {
                Some(id) => {
                    self.park_attach_reply(id, waiter);
                    true
                },
                None => {
                    if let Some(waiter) = waiter {
                        let message = self
                            .modals
                            .error_dialog
                            .clone()
                            .unwrap_or_else(|| "failed to attach the pane".to_string());
                        let _ = waiter.send(Err(message));
                    }
                    false
                },
            };
        }

        // Every one of herdr's app clients draws the same focused pane, so a
        // shared view shows a row's pane only while herdr is focused there.
        // The attach focuses it so the first frame is already right, and
        // `sync_herdr_view_focus` focuses it again whenever the session comes
        // back up, which is what lets a side hold one session per row.
        if let Some(pending) = self.herdr.pending_attach.iter_mut().find(|p| p.key == key) {
            pending.waiters.extend(waiter);
            return true;
        }
        // The gesture is two herdr processes whatever `async_session_spawn`
        // says, and running them from the click would hold the frame for as
        // long as herdr takes to answer.
        self.herdr.pending_attach.push(PendingHerdrAttach {
            job: None,
            target,
            key,
            workspace,
            previous,
            waiters: waiter.into_iter().collect(),
        });
        ctx.request_repaint();
        true
    }

    /// Answer an attach once the session's PTY is live.  A client that
    /// attached in order to read the pane would otherwise be handed an id
    /// before anything behind it can answer.
    pub(super) fn park_attach_reply(
        &mut self,
        id: SessionId,
        waiter: Option<mpsc::Sender<ipc::IpcResult>>,
    ) {
        let Some(waiter) = waiter else { return };
        if let Some(waiter) = self.pending_spawns.watch(id, waiter) {
            let _ = waiter.send(Ok(json!({ "session_id": id })));
        }
    }

    /// Open a session on every listed pane no session holds yet.  The listing
    /// is the one the sidebar and the palette both draw, so this opens exactly
    /// the rows the user could have opened one at a time.
    ///
    /// The batch was asked for the whole set rather than for one pane, so it
    /// switches to none of them: each session files under the workspace its
    /// own pane belongs to and the workspace on screen is left alone.
    pub(super) fn attach_every_multiplexer_pane(&mut self, ctx: &Context) {
        if !self.config.integrations.herdr.enabled {
            self.modals.error_dialog = Some(HERDR_DISABLED.to_string());
            return;
        }
        let panes: Vec<(herdr::HerdrKey, String, WorkspaceKey)> = self
            .herdr_agent_listing()
            .into_iter()
            .map(|(workspace, side, agent)| {
                let key =
                    herdr::HerdrKey { side: side.clone(), terminal_id: agent.terminal_id.clone() };
                (key, agent.pane_id.clone(), workspace)
            })
            .collect();
        for (key, pane_id, workspace) in panes {
            // Naming the pane's own workspace as the one to restore makes
            // both arms of the restore no-ops, so a refusal cannot move a
            // user who navigated while the gesture was still running.
            let previous = workspace.clone();
            let unlisted = unlisted_pane_target(&key, &pane_id);
            self.attach_herdr_agent(ctx, key, unlisted, workspace, previous, None);
        }
    }

    /// End every session attached to a multiplexer pane.  The panes keep
    /// running and their rows come back unattached, so this destroys nothing.
    pub(super) fn detach_every_multiplexer_pane(&mut self, ctx: &Context) {
        if !self.config.integrations.herdr.enabled {
            self.modals.error_dialog = Some(HERDR_DISABLED.to_string());
            return;
        }
        let ids = self.multiplexer_session_ids();
        if ids.is_empty() {
            return;
        }
        // `confirm_session_detach` governs one detach, and asking it per
        // session would put the same dialog in front of the user once per
        // row; the batch is one gesture and asks once.
        if self.config.ui.confirm_session_detach {
            self.modals.pending_detach_all = Some(ids);
        } else {
            self.detach_sessions(ctx, &ids);
        }
    }

    /// Every session holding a multiplexer pane, as ids: closing mutates
    /// `self.sessions`, so the set a detach acts on has to be taken before
    /// the first close rather than walked as it shrinks.
    pub(super) fn multiplexer_session_ids(&self) -> Vec<SessionId> {
        self.sessions.iter().filter(|s| s.herdr_key.is_some()).map(|s| s.id).collect()
    }

    pub(super) fn detach_sessions(&mut self, ctx: &Context, ids: &[SessionId]) {
        for id in ids {
            self.close_session(ctx, *id);
        }
    }

    /// Ask the multiplexer for a pane and open a session on it once it
    /// answers.  The two halves cannot be one call: herdr is a process, and
    /// the pane an attach needs does not exist until it answers.
    pub(super) fn create_multiplexer_pane(
        &mut self,
        ctx: &Context,
        side: herdr::Side,
        workspace: WorkspaceKey,
        waiter: Option<mpsc::Sender<ipc::IpcResult>>,
    ) {
        let cwd = match multiplexer_cwd(&side, workspace.as_deref()) {
            Ok(cwd) => cwd,
            Err(e) => {
                self.refuse_herdr_create(waiter, e);
                return;
            },
        };
        // `Multiplexer::owning` resolves a multiplexer from a pane's key,
        // and a pane nothing has made yet has no key, so the create names the
        // one it is asking.
        let multiplexer = Multiplexer::from(Herdr);
        let asked = side.clone();
        let job = jobs::pool().spawn(jobs::Priority::Interactive, move |_blocking| {
            multiplexer.create_pane(&asked, cwd)
        });
        self.herdr.pending_create.push(PendingHerdrCreate { job, side, workspace, waiter });
        ctx.request_repaint();
    }

    /// Adopt the creates herdr has answered, handing each pane to the same
    /// attach a click takes.  The workspace is switched to first, so the
    /// session and any refusal are both readable where they were asked for.
    pub(super) fn poll_herdr_create(&mut self, ctx: &Context) {
        if self.herdr.pending_create.is_empty() {
            return;
        }
        let pending = self.herdr.pending_create.remove(0);
        match pending.job.poll() {
            Some(Ok(pane)) => {
                // `tab create` starts a shell, so nothing is in the pane for
                // herdr's agent registry to resolve until an agent starts
                // there, and until then the tab is its only handle.
                let unlisted = PaneTarget {
                    side: pending.side.clone(),
                    pane_id: pane.pane_id,
                    tab_id: Some(pane.tab_id),
                    has_agent: false,
                };
                let key = herdr::HerdrKey { side: pending.side, terminal_id: pane.terminal_id };
                let previous =
                    std::mem::replace(&mut self.current_workspace, pending.workspace.clone());
                if !self.attach_herdr_agent(
                    ctx,
                    key,
                    unlisted,
                    pending.workspace,
                    previous.clone(),
                    pending.waiter,
                ) {
                    self.current_workspace = previous;
                }
            },
            Some(Err(e)) => self.refuse_herdr_create(pending.waiter, e),
            None if pending.job.failed() => {
                self.refuse_herdr_create(
                    pending.waiter,
                    "the herdr pane create did not finish".to_string(),
                );
            },
            None => self.herdr.pending_create.insert(0, pending),
        }
    }

    /// Report a create that made no pane, whether the multiplexer refused it
    /// or it was refused before the multiplexer was asked.  Nothing has
    /// switched workspace yet, since that waits for the pane to land, so a
    /// refusal leaves the user where they are and only has to be readable.
    pub(super) fn refuse_herdr_create(
        &mut self,
        waiter: Option<mpsc::Sender<ipc::IpcResult>>,
        message: String,
    ) {
        if let Some(waiter) = waiter {
            let _ = waiter.send(Err(message.clone()));
        }
        self.modals.error_dialog = Some(message);
    }

    /// Adopt the shared-view attaches whose herdr calls have landed.  Each
    /// session opens in the workspace its own click came from, which that
    /// click switched to before handing the gesture over.
    pub(super) fn poll_herdr_attach(&mut self, ctx: &Context) {
        if self.herdr.view_focus.is_some() || self.herdr.pending_attach.is_empty() {
            return;
        }
        // Attach gestures and session switches both change herdr's global
        // focus, so only one may be in flight.
        let mut pending = self.herdr.pending_attach.remove(0);
        if let Some(job) = &pending.job {
            match job.poll() {
                Some(Ok(launch)) => {
                    // The open takes the workspace by value, so the arm keeps
                    // its own copy to judge the restore against afterwards.
                    let switched_to = pending.workspace.clone();
                    let waiters = std::mem::take(&mut pending.waiters);
                    match self.open_herdr_session(
                        ctx,
                        pending.key,
                        pending.workspace,
                        launch.program,
                        launch.argv,
                        true,
                    ) {
                        Some(id) => {
                            for waiter in waiters {
                                self.park_attach_reply(id, Some(waiter));
                            }
                        },
                        None => {
                            self.restore_after_failed_attach(&switched_to, pending.previous);
                            let message = self.modals.error_dialog.clone().unwrap_or_default();
                            for waiter in waiters {
                                let _ = waiter.send(Err(message.clone()));
                            }
                        },
                    }
                },
                Some(Err(e)) => {
                    self.restore_after_failed_attach(&pending.workspace, pending.previous);
                    for waiter in std::mem::take(&mut pending.waiters) {
                        let _ = waiter.send(Err(e.clone()));
                    }
                    self.modals.error_dialog = Some(e);
                },
                None if job.failed() => {
                    self.restore_after_failed_attach(&pending.workspace, pending.previous);
                    let message = "the herdr attach did not finish".to_string();
                    for waiter in std::mem::take(&mut pending.waiters) {
                        let _ = waiter.send(Err(message.clone()));
                    }
                    self.modals.error_dialog = Some(message);
                },
                None => self.herdr.pending_attach.insert(0, pending),
            }
        } else {
            let name = self.herdr_session_name(&pending.key.side);
            let side = pending.key.side.clone();
            let target = self
                .find_herdr_agent(&side, &pending.key.terminal_id)
                .map_or_else(|| pending.target.clone(), |agent| agent.target(&side));
            let multiplexer = Multiplexer::owning(&pending.key);
            pending.job = Some(jobs::pool().spawn(jobs::Priority::Interactive, move |_blocking| {
                multiplexer.shared_view_gesture(&target, name)
            }));
            self.herdr.pending_attach.insert(0, pending);
        }
        if self.herdr.pending_attach.first().is_some_and(|pending| pending.job.is_none()) {
            ctx.request_repaint();
        }
    }

    pub(super) fn restore_after_failed_attach(
        &mut self,
        switched_to: &WorkspaceKey,
        previous: WorkspaceKey,
    ) {
        self.current_workspace =
            workspace_after_failed_attach(&self.current_workspace, switched_to, previous);
    }

    /// Open the session that runs an attach client.  A shared view starts on
    /// the pane the gesture just focused, so herdr is already where the new
    /// session's row says it is and no second focus is owed.
    ///
    /// `shared_view` is the caller's to say, since it chose the client: the
    /// listing may no longer say what it said then, or may not carry the pane
    /// at all.
    pub(super) fn open_herdr_session(
        &mut self,
        ctx: &Context,
        key: herdr::HerdrKey,
        workspace: WorkspaceKey,
        program: String,
        argv: Vec<String>,
        shared_view: bool,
    ) -> Option<SessionId> {
        // `alacritty_terminal::tty::Shell`'s fields are crate-private, so
        // this goes through the constructor rather than a struct literal.
        let shell = Shell::new(program, argv);
        match self.spawn_session_with_shell(ctx, workspace, Some(shell), None) {
            Ok(id) => {
                if let Some(session) = self.sessions.iter_mut().find(|s| s.id == id) {
                    session.bind_herdr(key.clone(), shared_view);
                }
                if shared_view {
                    self.herdr.focused_view.attached(id, Some(&key), Instant::now());
                }
                Some(id)
            },
            Err(e) => {
                self.modals.error_dialog = Some(format!("failed to attach herdr agent: {e}"));
                None
            },
        }
    }

    /// Local session switches focus herdr; a settled view follows later
    /// focus changes made inside herdr. Listings started before our focus
    /// landed cannot reverse the user's session selection.
    pub(super) fn sync_herdr_view_focus(&mut self, ctx: &Context) {
        let active = self.active_session_index().map(|index| &self.sessions[index]);
        let key = active.and_then(|session| session.herdr_key.clone());
        let selection = active.map(|session| (session.id, key.as_ref(), session.herdr_shared_view));
        let attentive = self.focus == PaneFocus::Terminal
            && !self.is_modal_open()
            && !self.palette.is_open()
            && ctx.input(|input| input.viewport().focused).unwrap_or(true);
        let action = self.herdr.focused_view.next(herdr::ViewInputs {
            active: selection,
            follow: self.config.integrations.herdr.follow_focus,
            caches: self.herdr.endpoints.caches(),
            attentive,
            busy: self.herdr.view_focus.is_some() || !self.herdr.pending_attach.is_empty(),
            now: Instant::now(),
            last_direct_input: self.last_direct_input,
        });
        if let Some(pending) = self.herdr.view_focus.take() {
            if !self.sessions.iter().any(|session| session.id == pending.session) {
                ctx.request_repaint();
                return;
            }
            match pending.job.poll() {
                Some(result) => {
                    let succeeded = result.is_ok();
                    if let Err(e) = result {
                        log::warn!("{e}");
                    }
                    if succeeded {
                        self.herdr.focused_view.moved_focus(&pending.key, Instant::now());
                    }
                    self.herdr.focused_view.settled(pending.session, succeeded, Instant::now());
                },
                None if pending.job.failed() => {
                    self.herdr.focused_view.settled(pending.session, false, Instant::now());
                },
                None => self.herdr.view_focus = Some(pending),
            }
            if self.herdr.view_focus.is_none() {
                ctx.request_repaint();
            }
            return;
        }
        match action {
            Some(herdr::HerdrViewAction::Focus(id)) => {
                let Some(key) = key else { return };
                let Some(focus) =
                    self.herdr_focus_target(&key).map(|target| herdr::focus_args(&target))
                else {
                    return;
                };
                let stamp = key.clone();
                let job = jobs::pool().spawn(jobs::Priority::Interactive, move |_blocking| {
                    herdr::focus_pane(&key.side, &focus)
                });
                self.herdr.view_focus =
                    Some(herdr::HerdrViewFocus { session: id, key: stamp, job });
            },
            Some(herdr::HerdrViewAction::Follow(key)) => self.follow_herdr_view(ctx, key),
            None => {},
        }
    }

    pub(super) fn reconcile_herdr_sessions(&mut self, ctx: &Context) {
        if !self.config.integrations.herdr.enabled {
            return;
        }
        let mut index = 0;
        while index < self.sessions.len() {
            let session = &self.sessions[index];
            let evidence = session.herdr_key.as_ref().zip(session.herdr_bound_at).and_then(
                |(key, bound_at)| {
                    self.herdr
                        .endpoints
                        .caches()
                        .iter()
                        .find(|cache| cache.side() == &key.side)
                        .and_then(herdr::EndpointCache::inventory)
                        .filter(|inventory| {
                            inventory.sampled_at > bound_at
                                && !inventory.terminal_ids.contains(&key.terminal_id)
                        })
                        .map(|inventory| (key, bound_at, inventory.sampled_at))
                },
            );
            if let Some((key, bound_at, sampled_at)) = evidence {
                let id = session.id;
                log::debug!(
                    "herdr removal session={id} side={:?} terminal_id={} bound_at={bound_at:?} \
                     sampled_at={sampled_at:?}",
                    key.side,
                    key.terminal_id
                );
                self.close_session(ctx, id);
            } else {
                index += 1;
            }
        }
    }

    pub(super) fn follow_herdr_view(&mut self, ctx: &Context, key: herdr::HerdrKey) {
        let Some(id) = self.herdr_follow_target(ctx, &key) else {
            // herdr keeps reporting this pane as focused, so a target the app
            // cannot reach is proposed again on every poll until the refusal
            // is on the trail.
            self.herdr.focused_view.moved_focus(&key, Instant::now());
            return;
        };
        self.activate_session_by_id(id);
        self.reveal_search_row(&SidebarRow::Session(id));
        self.set_sidebar_cursor(SidebarRow::Session(id));
        self.focus_terminal();
        self.herdr.focused_view.attached(id, Some(&key), Instant::now());
    }

    /// The session showing `key`, opening one if the row is attachable.
    pub(super) fn herdr_follow_target(
        &mut self,
        ctx: &Context,
        key: &herdr::HerdrKey,
    ) -> Option<SessionId> {
        if let Some(id) = self.herdr_session_for(key) {
            return Some(id);
        }
        let workspace = self.herdr_row_workspace(&key.side, &key.terminal_id)?;
        let shared_view = !self.herdr_attaches_directly(key);
        let (program, argv) = if !shared_view {
            let agent = self.find_herdr_agent(&key.side, &key.terminal_id)?;
            let attach = self.config.integrations.herdr.attach;
            // The branch already asked the question the multiplexer answers
            // here, so the `None` is unreachable; not following the pane is
            // the right answer anyway if the two ever disagree.
            let launch = Multiplexer::owning(key)
                .open_multiplexer_session(&agent.target(&key.side), attach)?;
            (launch.program, launch.argv)
        } else {
            let name = self.herdr_session_name(&key.side)?;
            key.side.command(herdr::PROGRAM, &["session", "attach", &name])
        };
        self.open_herdr_session(ctx, key.clone(), workspace, program, argv, shared_view)?;
        self.herdr_session_for(key)
    }

    /// `listed_herdr_agents` against this frame's own state.  Empty while herdr
    /// is disabled, so a caller never has to ask twice.
    pub(super) fn herdr_agent_listing(&self) -> Vec<(WorkspaceKey, &herdr::Side, &herdr::Agent)> {
        if !self.config.integrations.herdr.enabled {
            return Vec::new();
        }
        let claimed: Vec<herdr::HerdrKey> =
            self.sessions.iter().filter_map(|s| s.herdr_key.clone()).collect();
        let workspaces = herdr_workspaces(&self.projects, |path| self.liveness.missing(path));
        listed_herdr_agents(
            self.herdr.endpoints.caches(),
            &claimed,
            &workspaces,
            self.config.integrations.herdr.show_unmatched,
        )
    }

    /// Where a pane sits in herdr's own listing, counted across endpoints in
    /// the order they are polled.  `None` for a pane no endpoint lists.
    pub(super) fn herdr_pane_index(&self, key: &herdr::HerdrKey) -> Option<usize> {
        let mut before = 0;
        for cache in self.herdr.endpoints.caches() {
            if cache.side() == &key.side {
                let at = cache.agents().iter().position(|a| a.terminal_id == key.terminal_id)?;
                return Some(before + at);
            }
            before += cache.agents().len();
        }
        None
    }

    /// The workspace a herdr row is currently listed under, for the keyboard
    /// path: `SidebarRow::HerdrAgent` itself carries no workspace, unlike a
    /// click, which already knows which panel section it landed in.
    pub(super) fn herdr_row_workspace(
        &self,
        side: &herdr::Side,
        terminal_id: &str,
    ) -> Option<WorkspaceKey> {
        let wanted = sidebar_nav::WorkspaceEntry::Agent(side.clone(), terminal_id.to_string());
        self.listed_workspace_rows()
            .into_iter()
            .find(|(_, entries)| entries.contains(&wanted))
            .map(|(ws, _)| ws)
    }

    /// Where herdr's focus goes for the pane a session is bound to.  The
    /// displayed listing drops a pane with no agent in it unless panes are
    /// shown, so a bound shell pane is found through what the side's full
    /// listing last said about it.
    pub(super) fn herdr_focus_target(&self, key: &herdr::HerdrKey) -> Option<PaneTarget> {
        let cache = self.herdr.endpoints.caches().iter().find(|cache| cache.side() == &key.side)?;
        cache
            .agents()
            .iter()
            .find(|agent| agent.terminal_id == key.terminal_id)
            .or_else(|| cache.attachment_pane(&key.terminal_id).map(|pane| &pane.agent))
            .map(|agent| agent.target(&key.side))
    }

    /// The agent behind `(side, terminal_id)`, if its endpoint still has it
    /// cached.  A stale key (the agent exited between poll and paint, or an
    /// Enter that outraced this frame's own listing) yields no row rather
    /// than a panic; the next poll drops it from the listing for good.
    pub(super) fn find_herdr_agent(
        &self,
        side: &herdr::Side,
        terminal_id: &str,
    ) -> Option<&herdr::Agent> {
        self.herdr
            .endpoints
            .caches()
            .iter()
            .find(|cache| cache.side() == side)
            .and_then(|cache| cache.agents().iter().find(|a| a.terminal_id == terminal_id))
    }

    /// herdr's word on a session's agent: `Some` only while this session is
    /// attached to one the endpoint listing still carries.
    pub(super) fn session_herdr_status(&self, session: &Session) -> Option<herdr::Status> {
        self.session_herdr_agent(session).and_then(|agent| agent.status)
    }

    /// The agent this session is attached to, while the endpoint listing
    /// still carries it.  herdr watches the pane from outside, so it is the
    /// authority on both what the pane is called and what it is doing.
    pub(super) fn session_herdr_agent(&self, session: &Session) -> Option<&herdr::Agent> {
        let key = session.herdr_key.as_ref()?;
        self.find_herdr_agent(&key.side, &key.terminal_id)
    }

    /// Whether herdr reports an agent in the pane `key` names.  A pane the
    /// listing no longer carries answers true, as does a session that is not
    /// herdr's at all: with nothing to read, the agent-registry answer is the
    /// one that keeps every caller on the path it took before the pane went.
    pub(super) fn herdr_pane_has_agent(&self, key: Option<&herdr::HerdrKey>) -> bool {
        key.and_then(|key| self.find_herdr_agent(&key.side, &key.terminal_id))
            .is_none_or(|agent| agent.status.is_some())
    }

    /// Whether opening this pane's row attaches to the pane on its own.
    pub(super) fn herdr_attaches_directly(&self, key: &herdr::HerdrKey) -> bool {
        herdr::attaches_directly(
            &key.side,
            self.config.integrations.herdr.attach,
            self.herdr_pane_has_agent(Some(key)),
        )
    }

    /// What the endpoint learned this side's herdr session is called.
    pub(super) fn herdr_session_name(&self, side: &herdr::Side) -> Option<String> {
        self.herdr
            .endpoints
            .caches()
            .iter()
            .find(|cache| cache.side() == side)
            .and_then(herdr::EndpointCache::session_name)
    }

    /// Where a herdr-backed session lives, looked up from the key the session
    /// carries.
    pub(super) fn session_multiplexer_json(&self, key: &herdr::HerdrKey) -> Value {
        multiplexer_json(
            &key.side,
            &key.terminal_id,
            self.herdr_session_name(&key.side),
            self.find_herdr_agent(&key.side, &key.terminal_id),
        )
    }

    /// Every pane herdr reports, on every side, attached or not.  Unlike the
    /// sidebar this hides nothing: `show_unmatched` decides what is worth
    /// drawing, and a caller naming a pane by its id is not browsing.
    pub(super) fn multiplexer_panes_json(&self) -> Value {
        if !self.config.integrations.herdr.enabled {
            return json!({ "panes": [] });
        }
        let workspaces = herdr_workspaces(&self.projects, |path| self.liveness.missing(path));
        let mut panes = Vec::new();
        for cache in self.herdr.endpoints.caches() {
            let side = cache.side();
            let session = cache.session_name();
            for agent in cache.agents() {
                let key =
                    herdr::HerdrKey { side: side.clone(), terminal_id: agent.terminal_id.clone() };
                panes.push(json!({
                    "multiplexer": multiplexer_json(
                        side,
                        &agent.terminal_id,
                        session.clone(),
                        Some(agent),
                    ),
                    "kind": agent.kind,
                    "title": agent.title,
                    "status": agent.status.map(|status| status.label()),
                    "focused": agent.focused,
                    "workspace": herdr::match_workspace(agent, side, &workspaces),
                    "session_id": self.herdr_session_for(&key),
                }));
            }
        }
        json!({ "panes": panes })
    }

    /// The session already attached to this agent, if one is open.
    pub(super) fn herdr_session_for(&self, key: &herdr::HerdrKey) -> Option<SessionId> {
        self.sessions.iter().find(|s| s.herdr_key.as_ref() == Some(key)).map(|s| s.id)
    }

    /// herdr's detach chord on `side`, once that endpoint's config read has
    /// landed.
    pub(super) fn herdr_settings(&self, side: &herdr::Side) -> herdr::Settings {
        self.herdr
            .endpoints
            .caches()
            .iter()
            .find(|cache| cache.side() == side)
            .map(herdr::EndpointCache::settings)
            .unwrap_or_default()
    }

    /// What supervises `session`, when anything does.  Derived per frame
    /// rather than stored, so a config read that lands later, or a herdr that
    /// stops listing the agent, reaches the row without a second source of
    /// truth to keep in step.
    pub(super) fn session_managed(&self, session: &Session) -> Option<Managed> {
        let key = session.herdr_key.as_ref()?;
        let agent = self.session_herdr_agent(session);
        let mut managed = Managed::herdr(
            &key.side,
            &self.herdr_settings(&key.side),
            self.config.integrations.herdr.attach,
            agent,
        );
        // The listing answers what opening the pane now would give; this
        // session's client was settled when it attached.
        managed.shared_view = session.herdr_shared_view;
        Some(managed)
    }

    /// One number standing for every endpoint's rendered state, so the
    /// sidebar's per-frame comparison stays a `u64` compare.
    pub(super) fn herdr_generation(&self) -> u64 {
        self.herdr.endpoints.generation()
    }

    /// Refreshes the herdr endpoints on their own clock; a no-op per endpoint
    /// until its poll interval elapses.  Disabled stops the polling, not just
    /// the rows: the subprocesses are the whole cost of the feature, so an
    /// opt-out that kept running them would opt out of nothing.
    pub(super) fn poll_herdr_endpoints(&mut self) {
        if !self.config.integrations.herdr.enabled {
            return;
        }
        self.herdr.endpoints.poll(
            self.config.integrations.herdr.poll_interval,
            herdr::Listing::wanted(self.config.integrations.herdr.show_panes),
            |side| {
                self.sessions
                    .iter()
                    .any(|session| session.herdr_key.as_ref().is_some_and(|key| &key.side == side))
            },
        );
    }

    /// Attach to a pane the way its sidebar row does, holding the reply until
    /// the session behind it can be read.  A refusal names what went wrong
    /// precisely enough to act on: a side that names no server, a pane no
    /// endpoint is reporting, and an integration that is switched off are
    /// three different situations, and only the last is worth retrying after
    /// a config change.
    pub(super) fn defer_attach_multiplexer_pane(
        &mut self,
        ctx: &Context,
        side: &str,
        terminal_id: &str,
        reply_tx: mpsc::Sender<ipc::IpcResult>,
    ) {
        if !self.config.integrations.herdr.enabled {
            let _ = reply_tx.send(Err(HERDR_DISABLED.to_string()));
            return;
        }
        let Some(parsed_side) = herdr::Side::parse(side) else {
            let _ = reply_tx.send(Err(not_a_side(side)));
            return;
        };
        let Some(agent) = self.find_herdr_agent(&parsed_side, terminal_id) else {
            let _ = reply_tx.send(Err(format!(
                "no pane `{terminal_id}` on {side}, see list_multiplexer_panes"
            )));
            return;
        };
        let pane_id = agent.pane_id.clone();
        let key =
            herdr::HerdrKey { side: parsed_side.clone(), terminal_id: terminal_id.to_string() };
        let workspaces = herdr_workspaces(&self.projects, |path| self.liveness.missing(path));
        let workspace = herdr::match_workspace(agent, &parsed_side, &workspaces);

        let previous = std::mem::replace(&mut self.current_workspace, workspace.clone());
        let unlisted = unlisted_pane_target(&key, &pane_id);
        if !self.attach_herdr_agent(ctx, key, unlisted, workspace, previous.clone(), Some(reply_tx))
        {
            self.current_workspace = previous;
        }
    }

    /// Create a pane and open a session on it.  An omitted side is the one
    /// the active session's own pane belongs to, since a user asking for
    /// another pane while looking at one means another like it; with no herdr
    /// session in front of them there is no such answer, so a machine
    /// reaching more than one server has to say which.
    pub(super) fn defer_create_multiplexer_pane(
        &mut self,
        ctx: &Context,
        side: Option<&str>,
        workspace: Option<PathBuf>,
        reply_tx: mpsc::Sender<ipc::IpcResult>,
    ) {
        if !self.config.integrations.herdr.enabled {
            let _ = reply_tx.send(Err(HERDR_DISABLED.to_string()));
            return;
        }
        let side = match side {
            Some(name) => match herdr::Side::parse(name) {
                Some(side) => side,
                None => {
                    let _ = reply_tx.send(Err(not_a_side(name)));
                    return;
                },
            },
            None => match self.default_multiplexer_side() {
                Ok(side) => side,
                Err(e) => {
                    let _ = reply_tx.send(Err(e));
                    return;
                },
            },
        };
        // Resolved before herdr is asked, so a path naming no worktree never
        // leaves a pane behind in the multiplexer.
        let workspace = match workspace {
            None => self.current_workspace.clone(),
            Some(p) => match self.known_worktree_path(&p) {
                Some(known) => Some(known),
                None => {
                    let _ = reply_tx.send(Err(unknown_worktree(&p)));
                    return;
                },
            },
        };
        self.create_multiplexer_pane(ctx, side, workspace, Some(reply_tx));
    }

    /// The side a create that named none happens on: the one the active
    /// session's own pane belongs to, and failing that the one endpoint a
    /// server is answering on.  `Err` names every side it could have meant,
    /// so a caller can retry saying which.
    pub(super) fn default_multiplexer_side(&self) -> Result<herdr::Side, String> {
        let focused = self
            .active_session_index()
            .and_then(|idx| self.sessions[idx].herdr_key.as_ref())
            .map(|key| key.side.clone());
        if let Some(side) = focused {
            return Ok(side);
        }
        // A cache holds a sample time only while it still holds a listing, so
        // a side whose rows are gone is not offered as the one a create meant.
        let answering: Vec<&herdr::Side> = self
            .herdr
            .endpoints
            .caches()
            .iter()
            .filter(|cache| cache.sampled_at().is_some())
            .map(herdr::EndpointCache::side)
            .collect();
        match answering.as_slice() {
            [side] => Ok((*side).clone()),
            [] => Err("no herdr server is answering; start one, or name a side".to_string()),
            sides => Err(format!(
                "no herdr session is focused and {} are answering; name one",
                sides.iter().map(|side| side.name()).collect::<Vec<_>>().join(" and ")
            )),
        }
    }
}

impl AlacritreeApp {
    pub(super) fn dispatch_herdr_action(&mut self, ctx: &Context, action: NamedAction) -> bool {
        match action {
            NamedAction::NewMultiplexerPane => {
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
            NamedAction::AttachAllMultiplexerPanes => {
                self.attach_every_multiplexer_pane(ctx);
            },
            NamedAction::DetachAllMultiplexerPanes => {
                self.detach_every_multiplexer_pane(ctx);
            },
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_session_removes_only_its_attach_and_view_focus() {
        let mut herdr = HerdrGlue::new();
        let first_id = 1;
        let first_key = herdr::HerdrKey { side: herdr::Side::Native, terminal_id: "first".into() };
        let second_key =
            herdr::HerdrKey { side: herdr::Side::Native, terminal_id: "second".into() };
        let (first_tx, first_rx) = mpsc::channel();
        let (second_tx, second_rx) = mpsc::channel();

        herdr.pending_attach.push(PendingHerdrAttach {
            job: None,
            target: unlisted_pane_target(&first_key, "w1:p1"),
            key: first_key.clone(),
            workspace: None,
            previous: None,
            waiters: vec![first_tx],
        });
        herdr.pending_attach.push(PendingHerdrAttach {
            job: None,
            target: unlisted_pane_target(&second_key, "w1:p2"),
            key: second_key.clone(),
            workspace: None,
            previous: None,
            waiters: vec![second_tx],
        });
        herdr.view_focus = Some(herdr::HerdrViewFocus {
            session: first_id,
            key: first_key.clone(),
            job: jobs::Job::ready(Ok(())),
        });

        herdr.close_session(first_id, Some(&first_key));

        assert!(herdr.view_focus.is_none());
        assert_eq!(herdr.pending_attach.len(), 1);
        assert_eq!(herdr.pending_attach[0].key, second_key);
        assert_eq!(
            first_rx.try_recv().unwrap(),
            Err("the session behind this pane was closed before the attach finished".to_string())
        );
        assert!(second_rx.try_recv().is_err());
    }
}
