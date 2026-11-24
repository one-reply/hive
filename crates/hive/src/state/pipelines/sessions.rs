//! Each pending ticket owns a real PTY; views attach to an existing session on demand.
use super::*;
use crate::state::agents::Agents;
use crate::state::terminal_session::{SessionEvent, SessionStatus};
use hive_core::agent::AgentStatus;
use hive_core::agent::launch::{Launch, PipelineLaunch, PipelineRole};
use hive_core::pipeline::images;

/// What an agent's own report of its turn means for the step it runs.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum TurnOutcome {
    Failed,
    /// Asking the user is part of a step, so the attempt stays open until a stage report.
    AwaitsUser,
    InProgress,
}

pub(super) fn turn_outcome(status: AgentStatus) -> TurnOutcome {
    match status {
        AgentStatus::Failed => TurnOutcome::Failed,
        AgentStatus::Done | AgentStatus::WaitingInput | AgentStatus::WaitingApproval => {
            TurnOutcome::AwaitsUser
        }
        AgentStatus::Working | AgentStatus::Idle => TurnOutcome::InProgress,
    }
}

/// The attention status for a pending step whose live agent waits for the user.
fn request_label(status: AgentStatus) -> Option<&'static str> {
    match status {
        AgentStatus::WaitingApproval => Some("Agent needs approval"),
        AgentStatus::WaitingInput | AgentStatus::Done => Some("Agent waits for a reply"),
        AgentStatus::Working | AgentStatus::Idle | AgentStatus::Failed => None,
    }
}

impl Pipelines {
    fn agent_status(&self, ticket: &Ticket, agents: &Agents, cx: &App) -> Option<AgentStatus> {
        let session = &self.live.get(ticket)?.session;
        if session.read(cx).exited {
            return None;
        }
        agents.recipient(session.entity_id()).status
    }

    /// A step whose agent works again after a failed turn no longer needs a retry.
    pub(super) fn clear_recovered_sessions(&mut self, cx: &mut Context<Self>) {
        if self.session_errors.is_empty() {
            return;
        }
        let agents = self.agents.read(cx);
        let recovered: Vec<_> = self
            .session_errors
            .keys()
            .filter(|t| self.agent_status(t, agents, cx) == Some(AgentStatus::Working))
            .copied()
            .collect();
        if recovered.is_empty() {
            return;
        }
        for ticket in recovered {
            self.session_errors.remove(&ticket);
        }
        cx.notify();
    }

    /// Whether the attempt's live agent has stopped to ask the user something.
    pub(super) fn agent_waits(&self, ticket: &Ticket, cx: &App) -> bool {
        self.agent_status(ticket, self.agents.read(cx), cx)
            .and_then(request_label)
            .is_some()
    }

    pub(super) fn agent_request(&self, run: &Run, cx: &App) -> Option<&'static str> {
        let agents = self.agents.read(cx);
        run.pending()
            .iter()
            .filter_map(|t| self.agent_status(t, agents, cx))
            .find_map(request_label)
    }

    pub(crate) fn ticket_for_session(&self, id: EntityId) -> Option<Ticket> {
        self.live
            .iter()
            .find(|(_, v)| v.session.entity_id() == id)
            .map(|(t, _)| *t)
    }

    pub(crate) fn live_attempts(&self, id: u64) -> Vec<(Ticket, Phase)> {
        let mut attempts: Vec<_> = self
            .live
            .iter()
            .filter(|(ticket, _)| ticket.run == id)
            .map(|(ticket, live)| (*ticket, live.phase))
            .collect();
        attempts.sort_unstable_by_key(|(ticket, _)| ticket.attempt);
        attempts
    }

    pub(crate) fn session(&self, ticket: &Ticket) -> Option<Entity<TerminalSession>> {
        self.live.get(ticket).map(|live| live.session.clone())
    }

    /// Records why the attempt's session needs the user; `false` when it was already known.
    pub(super) fn session_failed(
        &mut self,
        ticket: Ticket,
        error: String,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.session_errors.get(&ticket) == Some(&error) {
            return false;
        }
        tracing::warn!(run=ticket.run, attempt=ticket.attempt,%error,"pipeline session needs attention");
        self.session_errors.insert(ticket, error);
        cx.notify();
        true
    }

    pub(super) fn stop_session(&mut self, ticket: &Ticket, cx: &mut Context<Self>) {
        if let Some(live) = self.live.remove(ticket) {
            if let Some(terminal) = live.session.read(cx).terminal() {
                self.runtime_mut(ticket.run)
                    .closing
                    .push(terminal.control());
            }
            // Return the accepted report to the CLI before closing its process tree.
            cx.defer(move |cx| live.session.update(cx, |s, cx| s.stop_pipeline(cx)));
        }
    }

    pub(super) fn dispatch(&mut self, id: u64, cx: &mut Context<Self>) -> Result<(), String> {
        let run = self.run(id, cx)?.clone();
        let role = match run.phase() {
            Phase::Planning => PipelineRole::Plan,
            Phase::Regression(hive_core::pipeline::RegressionStep::Author) | Phase::Development => {
                PipelineRole::Develop
            }
            Phase::Review(_) => PipelineRole::Review,
            _ => return Ok(()),
        };
        for ticket in run.pending() {
            if self.live.contains_key(&ticket) || self.suspended.contains(&ticket) {
                continue;
            }
            let profile = run
                .profile(&ticket)
                .ok_or("Invalid pipeline stage or lane.")?
                .clone();
            let first = ticket.lane.unwrap_or(0);
            let cwd = run
                .lanes()
                .get(first)
                .ok_or("No repository for this stage.")?
                .worktree
                .clone();
            let mut directories = Vec::new();
            if ticket.lane.is_none() {
                for lane in run.lanes() {
                    if lane.worktree != cwd && !directories.contains(&lane.worktree) {
                        directories.push(lane.worktree.clone());
                    }
                }
            }
            let (resume, prompt) = match self.resumes.remove(&ticket) {
                // The user's own Resume reopens the conversation as it was left.
                Some(session) => (Some(session), String::new()),
                None => match run.continuation(&ticket) {
                    Some((session, message)) => (Some(session.clone()), message),
                    None => (None, run.prompt(&ticket)?),
                },
            };
            let launch = PipelineLaunch {
                profile,
                role,
                directories,
                prompt,
                ticket,
                resume,
                image_directory: (!images::references(run.task()).is_empty())
                    .then(images::directory),
            };
            let session = cx.new(|cx| {
                TerminalSession::new(
                    Host::Local,
                    Some(cwd.clone()),
                    Some(cwd),
                    Some(Launch::Pipeline(launch)),
                    true,
                    cx,
                )
            });
            self.store.update(cx, |s, cx| {
                s.edit_pipelines(cx, |library| {
                    if let Some(record) = library.runs.iter_mut().find(|r| r.id() == id) {
                        let at = now_ms();
                        record.record_stage(None, at);
                        record.start_attempt_at(ticket, at);
                    }
                })
            });
            let subscriptions = vec![
                cx.observe(&session, move |p, session, cx| {
                    let s = session.read(cx);
                    let failure = match &s.status {
                        SessionStatus::Failed(e) => Some(e.clone()),
                        _ if s.exited => Some("Agent exited before reporting a result.".into()),
                        _ => None,
                    };
                    if let Some(error) = failure
                        && let Ok(run) = p.run(id, cx)
                        && run.pending().contains(&ticket)
                    {
                        if run.has_open_attempt(&ticket) {
                            p.store.update(cx, |s, cx| {
                                s.edit_pipelines(cx, |l| {
                                    if let Some(r) = l.runs.iter_mut().find(|r| r.id() == id) {
                                        r.finish_attempt_at(ticket, true, now_ms());
                                    }
                                })
                            });
                        }
                        // A failed turn is announced by the agent itself; a process that
                        // is gone has nobody left to do it.
                        if p.session_failed(ticket, error.clone(), cx) {
                            cx.emit(RunWaits {
                                run: id,
                                reason: Waiting::Failed(error),
                            });
                        }
                    }
                }),
                cx.subscribe(&session, move |p, session, event, cx| {
                    if let SessionEvent::DirectoryChanged = event {
                        p.capture(ticket, session, cx);
                    }
                }),
            ];
            self.live.insert(
                ticket,
                Live {
                    // The ticket's own phase: reviewers of a group each have a stage.
                    phase: run.ticket_phase(&ticket),
                    session: session.clone(),
                    _subscriptions: subscriptions,
                },
            );
            self.capture(ticket, session, cx);
            cx.notify();
        }
        Ok(())
    }

    fn capture(
        &mut self,
        ticket: Ticket,
        session: Entity<TerminalSession>,
        cx: &mut Context<Self>,
    ) {
        let Some(saved) = session.read(cx).resume.clone() else {
            return;
        };
        let Ok(run) = self.run(ticket.run, cx) else {
            return;
        };
        if !run.pending().contains(&ticket)
            || run
                .sessions()
                .iter()
                .any(|(t, s)| *t == ticket && *s == saved)
        {
            return;
        }
        self.store.update(cx, |s, cx| {
            s.edit_pipelines(cx, |library| {
                if let Some(run) = library.runs.iter_mut().find(|r| r.id() == ticket.run) {
                    run.record_session(ticket, saved);
                }
            })
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentStatus, TurnOutcome, request_label, turn_outcome};

    #[test]
    fn a_finished_turn_waits_for_the_user_instead_of_failing_the_step() {
        assert_eq!(turn_outcome(AgentStatus::Done), TurnOutcome::AwaitsUser);
        assert_eq!(
            turn_outcome(AgentStatus::WaitingInput),
            TurnOutcome::AwaitsUser
        );
        assert_eq!(
            turn_outcome(AgentStatus::WaitingApproval),
            TurnOutcome::AwaitsUser
        );
        assert_eq!(turn_outcome(AgentStatus::Failed), TurnOutcome::Failed);
        assert_eq!(turn_outcome(AgentStatus::Working), TurnOutcome::InProgress);
        assert_eq!(turn_outcome(AgentStatus::Idle), TurnOutcome::InProgress);
    }

    #[test]
    fn only_requests_to_the_user_need_attention() {
        assert_eq!(
            request_label(AgentStatus::Done),
            Some("Agent waits for a reply")
        );
        assert_eq!(
            request_label(AgentStatus::WaitingInput),
            Some("Agent waits for a reply")
        );
        assert_eq!(
            request_label(AgentStatus::WaitingApproval),
            Some("Agent needs approval")
        );
        for status in [AgentStatus::Working, AgentStatus::Idle, AgentStatus::Failed] {
            assert_eq!(request_label(status), None);
        }
    }
}
