//! Persist the next intent before a checkout, command or terminal is allowed to start.
use super::*;
use hive_core::agent::launch::{Launch, validate_pipeline_profile};
use hive_core::pipeline::CommandResult;

enum Work {
    Prepared(usize, PathBuf),
    Checked(Vec<CommandResult>),
    Dispatch,
    ReviewRevision(String),
}

impl Pipelines {
    pub(super) fn schedule(&mut self, id: u64, cx: &mut Context<Self>) {
        let generation = self.generations.entry(id).or_default();
        *generation = generation.wrapping_add(1);
        let generation = *generation;
        cx.notify();
        if self.stopped || self.errors.contains_key(&id) {
            return;
        }
        let Ok(run) = self.run(id, cx) else { return };
        if run.pending().is_empty() && !matches!(run.phase, Phase::Commands(_)) {
            return;
        }
        if !self.registered(&run.owner, cx)
            || run.lanes.iter().any(|l| !self.registered(&l.project, cx))
        {
            self.fail(
                id,
                "A participating project is no longer registered locally.".into(),
                cx,
            );
            return;
        }
        let run = run.clone();
        let saved = self.store.read(cx).pipeline_checkpoint();
        cx.spawn(async move |this, cx| {
            let saved = saved
                .recv()
                .await
                .unwrap_or_else(|_| Err("State writer stopped.".into()));
            let _ = this.update(cx, |p, cx| {
                if !p.current(id, generation) {
                    return;
                }
                match saved {
                    Ok(()) => p.work(run, generation, cx),
                    Err(error) => p.fail(
                        id,
                        format!("Pipeline checkpoint could not be saved: {error}"),
                        cx,
                    ),
                }
            });
        })
        .detach();
    }

    fn current(&self, id: u64, generation: u64) -> bool {
        !self.stopped
            && self.generations.get(&id) == Some(&generation)
            && !self.errors.contains_key(&id)
    }

    fn work(&mut self, run: Run, generation: u64, cx: &mut Context<Self>) {
        let id = run.id;
        let cancel = Arc::new(Cancellation::default());
        if let Some(old) = self.workers.insert(id, cancel.clone()) {
            old.cancel();
        }
        self.closing.retain(|_, list| {
            list.retain(|pty| !pty.is_closed());
            !list.is_empty()
        });
        let closing = self.closing.get(&id).cloned().unwrap_or_default();
        let work = cx.background_executor().spawn(async move {
            // A reporting agent may still have children writing while its PTY shuts down.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            if closing.iter().any(|pty| !pty.wait_until(deadline)) {
                return Err(
                    "A previous pipeline session is still closing. Retry when it has stopped."
                        .into(),
                );
            }
            if let Some((index, lane)) = run
                .lanes
                .iter()
                .enumerate()
                .find(|(_, l)| l.worktree.as_os_str().is_empty())
            {
                for profile in [&run.template.planner.profile]
                    .into_iter()
                    .chain(run.template.reviews.iter().map(|s| &s.profile))
                    .chain(run.lanes.iter().map(|l| &l.profile))
                {
                    validate_pipeline_profile(profile).map_err(|e| e.to_string())?;
                    Launch::New(profile.agent)
                        .check_executable(&Host::Local)
                        .map_err(|e| e.to_string())?;
                }
                if cancel.is_cancelled() {
                    return Err("Cancelled".into());
                }
                let path = workspace::prepare(id, lane, &cancel).map_err(|e| e.to_string())?;
                if run
                    .lanes
                    .iter()
                    .any(|other| other.worktree == path && other.base != lane.base)
                {
                    return Err(format!(
                        "Two lanes of {} require different base commits.",
                        lane.project.display()
                    ));
                }
                return Ok(Work::Prepared(index, path));
            }
            for lane in &run.lanes {
                if cancel.is_cancelled() {
                    return Err("Cancelled".into());
                }
                workspace::verify(id, lane).map_err(|e| e.to_string())?;
            }
            if run.phase == Phase::Review(0) && run.review_revision.is_none() {
                return workspace::revision(id, &run.lanes, &cancel)
                    .map(Work::ReviewRevision)
                    .map_err(|e| e.to_string());
            }
            if matches!(run.phase, Phase::Commands(_)) {
                return Ok(Work::Checked(workspace::checks(&run.lanes, &cancel)));
            }
            Ok(Work::Dispatch)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |p, cx| {
                // A user may cancel while Git is registering a checkout. Keep its actual path
                // visible for recovery even when this generation no longer schedules anything.
                if let Ok(Work::Prepared(index, path)) = &result {
                    p.store.update(cx, |s, cx| {
                        s.edit_pipelines(cx, |library| {
                            if let Some(run) = library.runs.iter_mut().find(|r| r.id == id)
                                && let Some(lane) = run.lanes.get_mut(*index)
                                && lane.worktree.as_os_str().is_empty()
                            {
                                lane.worktree = path.clone();
                            }
                        })
                    });
                    AppState::get(cx)
                        .projects
                        .clone()
                        .update(cx, |projects, cx| projects.rescan(cx).detach());
                }
                if !p.current(id, generation) {
                    return;
                }
                p.workers.remove(&id);
                let result = match result {
                    Err(error) => Err(error),
                    Ok(Work::Prepared(_, _)) => {
                        p.schedule(id, cx);
                        Ok(())
                    }
                    Ok(Work::Checked(results)) => {
                        p.edit(id, cx, |run| Ok(run.command_results(results)?))
                    }
                    Ok(Work::ReviewRevision(hash)) => p.edit(id, cx, |r| {
                        r.review_revision = Some(hash);
                        Ok(())
                    }),
                    Ok(Work::Dispatch) => p.dispatch(id, cx),
                };
                if let Err(error) = result {
                    p.fail(id, error, cx);
                }
            });
        })
        .detach();
    }

    pub(super) fn fail(&mut self, id: u64, reason: String, cx: &mut Context<Self>) {
        if self.errors.get(&id) != Some(&reason) {
            tracing::warn!(run=id,%reason,"pipeline needs attention");
            self.errors.insert(id, reason);
            cx.notify();
        }
    }

    pub(crate) fn retry(&mut self, id: u64, cx: &mut Context<Self>) -> Result<(), String> {
        if self.error(id).is_none() {
            return Err("There is no interrupted work to retry.".into());
        }
        if self.run(id, cx)?.phase == Phase::Acceptance {
            return self.accept(id, cx);
        }
        let mut run = self.run(id, cx)?.clone();
        let interrupted: Vec<_> = run
            .pending()
            .into_iter()
            .filter(|ticket| {
                self.session_errors.contains_key(ticket)
                    || self.session(ticket).is_none_or(|session| {
                        let session = session.read(cx);
                        session.exited
                            || matches!(
                                session.status,
                                super::super::terminal_session::SessionStatus::Failed(_)
                            )
                    })
            })
            .collect();
        for ticket in interrupted {
            run.restart_attempt(&ticket)?;
            timing::finish_attempt(&mut run, ticket, true, timing::now());
            self.suspended.remove(&ticket);
            self.session_errors.remove(&ticket);
            self.stop_session(&ticket, cx);
        }
        self.errors.remove(&id);
        self.edit(id, cx, |record| {
            *record = run;
            Ok(())
        })
    }

    pub(crate) fn resume(&mut self, ticket: &Ticket, cx: &mut Context<Self>) -> Result<(), String> {
        let run = self.run(ticket.run, cx)?;
        let session = run
            .sessions
            .iter()
            .find(|(t, _)| t == ticket)
            .map(|(_, s)| s.clone())
            .ok_or("The CLI did not save a resumable conversation.")?;
        if !run.pending().contains(ticket)
            || self.session(ticket).is_some_and(|s| {
                let s = s.read(cx);
                !s.exited
                    && !matches!(
                        s.status,
                        super::super::terminal_session::SessionStatus::Failed(_)
                    )
            })
        {
            return Err("This attempt cannot be resumed.".into());
        }
        self.stop_session(ticket, cx);
        self.suspended.remove(ticket);
        self.session_errors.remove(ticket);
        self.resumes.insert(*ticket, session);
        self.errors.remove(&ticket.run);
        self.schedule(ticket.run, cx);
        Ok(())
    }
}
