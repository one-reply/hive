//! Persist the next intent before a checkout, command or terminal is allowed to start.
use super::*;
use hive_core::agent::launch::{Launch, validate_pipeline_profile};
use hive_core::pipeline::CommandResult;
use runtime::Failure;

/// Whether the run has a step for Hive to start: an agent's attempt or a command of its own.
pub(super) fn has_work(run: &Run) -> bool {
    !run.pending().is_empty() || run.has_command_work()
}

/// Whether a decision replaces the failure recorded for its run. A run's error holds its
/// scheduled step back, so a decision that leaves work to do, or moves the run to another
/// phase, would be saved and then wait for a retry of a step nobody asks for any more. Only
/// an edit that stays in the phase and schedules nothing (a note on a criterion, one of
/// several findings dismissed) keeps the failure: a retry still repeats that step.
pub(super) fn replaces_failure(before: Phase, after: Phase, has_work: bool) -> bool {
    before != after || has_work
}

enum Work {
    Prepared(Vec<usize>, PathBuf),
    Checked(Vec<CommandResult>, Option<String>),
    Regression(hive_core::pipeline::workspace::RegressionResult),
    Dispatch,
    ReviewRevision((String, Vec<String>)),
}

impl Pipelines {
    pub(super) fn schedule(&mut self, id: u64, saved: Checkpoint, cx: &mut Context<Self>) {
        let generation = self.next_job();
        self.runtime_mut(id).generation = generation;
        cx.notify();
        if self.stopped {
            return;
        }
        cx.spawn(async move |this, cx| {
            let result = saved
                .recv()
                .await
                .unwrap_or_else(|_| Err("State writer stopped.".into()));
            let _ = this.update(cx, |p, cx| {
                if p.stopped || p.runtime(id).is_none_or(|r| r.generation != generation) {
                    return;
                }
                p.settle_reports(id, result.clone(), cx);
                if let Err(error) = result {
                    let reason = format!("Pipeline checkpoint could not be saved: {error}");
                    p.stop_for(id, Failure::Checkpoint(reason), cx);
                    return;
                }
                p.forget_unsaved_checkpoint(id, cx);
                if !p.current(id, generation) {
                    return;
                }
                let Ok(run) = p.run(id, cx) else { return };
                if !has_work(run) {
                    return;
                }
                if !p.registered(run.owner(), cx)
                    || run.lanes().iter().any(|l| !p.registered(&l.project, cx))
                {
                    p.fail(
                        id,
                        "A participating project is no longer registered locally.".into(),
                        cx,
                    );
                    return;
                }
                p.work(run.clone(), generation, cx);
            });
        })
        .detach();
    }

    fn current(&self, id: u64, generation: u64) -> bool {
        !self.stopped
            && self
                .runtime(id)
                .is_some_and(|r| r.generation == generation && r.error.is_none())
    }

    fn work(&mut self, run: Run, generation: u64, cx: &mut Context<Self>) {
        let id = run.id();
        let phase = run.phase();
        let cancel = Arc::new(Cancellation::default());
        let runtime = self.runtime_mut(id);
        if let Some(old) = runtime.worker.replace(cancel.clone()) {
            old.cancel();
        }
        runtime.closing.retain(|pty| !pty.is_closed());
        let closing = runtime.closing.clone();
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
                .lanes()
                .iter()
                .enumerate()
                .find(|(_, l)| l.worktree.as_os_str().is_empty())
            {
                for profile in (!run.direct_start())
                    .then_some(&run.template().planner.profile)
                    .into_iter()
                    .chain(run.template().reviews.iter().map(|s| &s.profile))
                    .chain(run.lanes().iter().map(|l| &l.profile))
                {
                    validate_pipeline_profile(profile).map_err(|e| e.to_string())?;
                    Launch::New(profile.agent)
                        .check_executable(&Host::Local)
                        .map_err(|e| e.to_string())?;
                }
                if cancel.is_cancelled() {
                    return Err("Cancelled".into());
                }
                let (members, path) =
                    workspace::prepare_lanes(&run, index, &cancel).map_err(|e| {
                        format!("Preparing {} ({}): {e}", lane.name, lane.project.display())
                    })?;
                return Ok(Work::Prepared(members, path));
            }
            for lane in run.lanes() {
                if cancel.is_cancelled() {
                    return Err("Cancelled".into());
                }
                workspace::verify(id, lane).map_err(|e| e.to_string())?;
            }
            if run.phase() == Phase::Review(0) && run.review_revision().is_none() {
                return workspace::reviewed_trees(id, run.lanes(), &cancel)
                    .map(Work::ReviewRevision)
                    .map_err(|e| e.to_string());
            }
            if matches!(run.phase(), Phase::Regression(_)) && run.has_command_work() {
                return workspace::run_regression(&run, &cancel)
                    .map(Work::Regression)
                    .map_err(|e| e.to_string());
            }
            if matches!(run.phase(), Phase::Commands(_)) {
                let before =
                    workspace::revision(id, run.lanes(), &cancel).map_err(|e| e.to_string())?;
                let checks = workspace::checks(run.lanes(), &cancel);
                let after =
                    workspace::revision(id, run.lanes(), &cancel).map_err(|e| e.to_string())?;
                return Ok(Work::Checked(checks, (before == after).then_some(after)));
            }
            Ok(Work::Dispatch)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |p, cx| {
                let current = p.current(id, generation);
                if current {
                    p.runtime_mut(id).worker = None;
                }
                let result = match result {
                    // Git may finish registering a checkout during cancellation. Keep its path visible.
                    Ok(Work::Prepared(members, path)) => {
                        let saved = p.store.update(cx, |s, cx| {
                            s.checkpoint_pipelines(cx, |library| {
                                if let Some(run) = library.runs.iter_mut().find(|r| r.id() == id) {
                                    for index in members {
                                        run.record_worktree(index, path.clone());
                                    }
                                }
                            })
                        });
                        AppState::get(cx)
                            .projects
                            .clone()
                            .update(cx, |projects, cx| projects.rescan(cx).detach());
                        if current {
                            p.schedule(id, saved, cx);
                        }
                        Ok(())
                    }
                    _ if !current => return,
                    Err(error) => Err(error),
                    Ok(Work::Regression(result)) => {
                        let applied = p.edit(id, cx, |run| Ok(run.record_regression(result)?));
                        p.announce_gate(id, phase, cx);
                        applied
                    }
                    Ok(Work::Checked(results, revision)) => {
                        let applied = p.edit(id, cx, |run| Ok(run.checked_at(results, revision)?));
                        p.announce_gate(id, phase, cx);
                        applied
                    }
                    Ok(Work::ReviewRevision((hash, trees))) => {
                        let applied = p.edit(id, cx, |r| Ok(r.record_review_trees(hash, trees)?));
                        p.announce_gate(id, phase, cx);
                        applied
                    }
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
        self.stop_for(id, Failure::Step(reason), cx);
    }

    /// Records why the run waits for the user and says so once per distinct failure.
    pub(super) fn stop_for(&mut self, id: u64, failure: Failure, cx: &mut Context<Self>) {
        let runtime = self.runtime_mut(id);
        if runtime.error.as_ref() != Some(&failure) {
            let reason = failure.message().to_owned();
            tracing::warn!(run=id,%reason,"pipeline needs attention");
            runtime.error = Some(failure);
            cx.emit(RunWaits {
                run: id,
                reason: Waiting::Failed(reason),
            });
            cx.notify();
        }
    }

    pub(crate) fn retry(&mut self, id: u64, cx: &mut Context<Self>) -> Result<(), String> {
        if self.error(id).is_none() {
            return Err("There is no interrupted work to retry.".into());
        }
        if self.ensure_reports_saved(id).is_err() {
            self.clear_error(id);
            let saved = self.store.read(cx).pipeline_checkpoint();
            self.schedule(id, saved, cx);
            return Ok(());
        }
        if matches!(
            self.runtime(id).and_then(|runtime| runtime.error.as_ref()),
            Some(Failure::Acceptance(_))
        ) {
            return self.accept(id, cx);
        }
        let mut run = self.run(id, cx)?.clone();
        let interrupted: Vec<_> = run
            .pending()
            .into_iter()
            .filter(|ticket| {
                // A new number fences off a process that may still report. A ticket that never
                // launched keeps its number, so the history lists only real retries.
                self.suspended.contains(ticket)
                    || self.session_errors.contains_key(ticket)
                    || match self.session(ticket) {
                        Some(session) => {
                            let session = session.read(cx);
                            session.exited
                                || matches!(
                                    session.status,
                                    super::super::terminal_session::SessionStatus::Failed(_)
                                )
                        }
                        None => run
                            .timings()
                            .iter()
                            .any(|row| row.attempt == Some(ticket.attempt)),
                    }
            })
            .collect();
        for ticket in interrupted {
            run.restart_attempt(&ticket)?;
            run.finish_attempt_at(ticket, true, now_ms());
            self.suspended.remove(&ticket);
            self.session_errors.remove(&ticket);
            self.stop_session(&ticket, cx);
        }
        self.clear_error(id);
        self.edit(id, cx, |record| {
            *record = run;
            Ok(())
        })
    }

    pub(crate) fn resume(&mut self, ticket: &Ticket, cx: &mut Context<Self>) -> Result<(), String> {
        self.ensure_reports_saved(ticket.run)?;
        let run = self.run(ticket.run, cx)?;
        let session = run
            .sessions()
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
        self.clear_error(ticket.run);
        let saved = self.store.read(cx).pipeline_checkpoint();
        self.schedule(ticket.run, saved, cx);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    // Not a glob import: `gpui_kit::*` brings in GPUI's `test` attribute, which shadows `#[test]`.
    use super::replaces_failure;
    use hive_core::pipeline::Phase;

    #[test]
    fn only_an_edit_that_stays_in_its_phase_and_schedules_nothing_keeps_the_failure() {
        let gate = Phase::Acceptance;
        assert!(!replaces_failure(gate, gate, false));
        assert!(replaces_failure(gate, Phase::Development, true));
        // Another gate: the failed step is behind the run although nothing starts.
        assert!(replaces_failure(Phase::Decision(0), gate, false));
        // Work in the same phase must never wait behind the old error.
        assert!(replaces_failure(
            Phase::Development,
            Phase::Development,
            true
        ));
        assert!(replaces_failure(Phase::Review(0), Phase::Review(1), true));
    }
}
