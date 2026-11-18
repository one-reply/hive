//! Each pending ticket owns a real PTY; views attach to an existing session on demand.
use super::*;
use crate::state::terminal_session::{SessionEvent, SessionStatus};
use hive_core::agent::launch::{Launch, PipelineLaunch, PipelineRole};

impl Pipelines {
    pub(crate) fn ticket_for_session(&self, id: EntityId) -> Option<Ticket> {
        self.live
            .iter()
            .find(|(_, v)| v.session.entity_id() == id)
            .map(|(t, _)| *t)
    }

    pub(crate) fn session(&self, ticket: &Ticket) -> Option<Entity<TerminalSession>> {
        self.live.get(ticket).map(|live| live.session.clone())
    }

    pub(super) fn session_failed(&mut self, ticket: Ticket, error: String, cx: &mut Context<Self>) {
        if self.session_errors.get(&ticket) != Some(&error) {
            tracing::warn!(run=ticket.run, attempt=ticket.attempt,%error,"pipeline session needs attention");
            self.session_errors.insert(ticket, error);
            cx.notify();
        }
    }

    pub(super) fn stop_session(&mut self, ticket: &Ticket, cx: &mut Context<Self>) {
        if let Some(live) = self.live.remove(ticket) {
            if let Some(terminal) = live.session.read(cx).terminal() {
                self.closing
                    .entry(ticket.run)
                    .or_default()
                    .push(terminal.control());
            }
            // Return the accepted report to the CLI before closing its process tree.
            cx.defer(move |cx| live.session.update(cx, |s, cx| s.stop_pipeline(cx)));
        }
    }

    pub(super) fn dispatch(&mut self, id: u64, cx: &mut Context<Self>) -> Result<(), String> {
        let run = self.run(id, cx)?.clone();
        let role = match run.phase {
            Phase::Planning => PipelineRole::Plan,
            Phase::Development => PipelineRole::Develop,
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
                .lanes
                .get(first)
                .ok_or("No repository for this stage.")?
                .worktree
                .clone();
            let mut directories = Vec::new();
            if ticket.lane.is_none() {
                for lane in &run.lanes {
                    if lane.worktree != cwd && !directories.contains(&lane.worktree) {
                        directories.push(lane.worktree.clone());
                    }
                }
            }
            let resume = self.resumes.remove(&ticket);
            let prompt = if resume.is_some() {
                String::new()
            } else {
                run.prompt(&ticket)?
            };
            let launch = PipelineLaunch {
                profile,
                role,
                directories,
                prompt,
                ticket,
                resume,
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
                    if let Some(record) = library.runs.iter_mut().find(|r| r.id == id) {
                        let at = timing::now();
                        timing::stage(record, None, at);
                        timing::start_attempt(record, ticket, at);
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
                        if timing::attempt_open(run, ticket) {
                            p.store.update(cx, |s, cx| {
                                s.edit_pipelines(cx, |l| {
                                    if let Some(r) = l.runs.iter_mut().find(|r| r.id == id) {
                                        timing::finish_attempt(r, ticket, true, timing::now());
                                    }
                                })
                            });
                        }
                        p.session_failed(ticket, error, cx);
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
                .sessions
                .iter()
                .any(|(t, s)| *t == ticket && *s == saved)
        {
            return;
        }
        self.store.update(cx, |s, cx| {
            s.edit_pipelines(cx, |library| {
                if let Some(run) = library.runs.iter_mut().find(|r| r.id == ticket.run) {
                    run.sessions.retain(|(t, _)| *t != ticket);
                    run.sessions.push((ticket, saved));
                }
            })
        });
    }
}
