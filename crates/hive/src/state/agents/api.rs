//! Answers `hive agent …` calls from the terminals: list, start, send, read and wait.
//!
//! Quick calls are answered at once. `wait` and `start` are kept until the poll sees the
//! agent settle or appear; `send` is answered by the recipient's terminal once the message
//! is submitted.
use super::*;
use crate::state::terminal_session::{Message, Recipient, SessionStatus};
use hive_core::agent::api::{
    self as protocol, Address, AgentInfo, ApiError, ErrorCode, Incoming, Outcome, Request, Response,
};
use hive_core::agent::launch::AGENTS;

/// Enough for a long build log; more belongs in a file the agent reads itself.
const MAX_READ_LINES: usize = 2_000;

/// What the workspace needs to open a named agent beside its caller.
pub(crate) struct SpawnRequest {
    /// The caller's terminal; the new pane opens beside it.
    pub(crate) caller: EntityId,
    pub(crate) host: Host,
    pub(crate) worktree: PathBuf,
    pub(crate) kind: AgentKind,
    pub(crate) name: String,
    /// Open a new tab instead of splitting the caller's pane.
    pub(crate) tab: bool,
}

/// A `wait` or `send --wait` call kept until its agent settles.
pub(super) struct Waiting {
    session: EntityId,
    /// After `send --wait` the turn counts only once the agent was seen working.
    after_submit: bool,
    seen_working: bool,
    deadline: Instant,
    reply: Incoming,
}

/// A `start` call kept until its agent runs (and took its first task).
pub(super) struct Starting {
    name: String,
    kind: AgentKind,
    /// Set by the workspace once the pane exists.
    session: Option<WeakEntity<TerminalSession>>,
    /// The first task, already wrapped for the recipient.
    prompt: Option<String>,
    deadline: Instant,
    reply: Incoming,
}

/// A terminal a call names.
struct Found {
    session: Entity<TerminalSession>,
    id: EntityId,
}

/// What a kept call needs next.
#[derive(Debug, PartialEq)]
enum Verdict {
    Keep,
    Settled,
    NoAgent,
    Expired,
}

impl Agents {
    pub(super) fn handle(&mut self, incoming: Incoming, cx: &mut Context<Self>) {
        let from = incoming.call().from;
        let answer = match incoming.call().request.clone() {
            Request::List => Ok(Response::Agents {
                agents: self.infos(cx),
            }),
            Request::Read { target, lines } => self.read_terminal(&target, lines, cx),
            Request::Wait { target, timeout_ms } => match self.working_agent(&target, cx) {
                Ok(Some(session)) => {
                    self.waits.push(Waiting {
                        session,
                        after_submit: false,
                        seen_working: true,
                        deadline: deadline(timeout_ms, protocol::WAIT_TIMEOUT),
                        reply: incoming,
                    });
                    return;
                }
                Ok(None) => self.agent_response(&target, cx),
                Err(error) => Err(error),
            },
            Request::Send {
                to,
                text,
                wait,
                timeout_ms,
            } => {
                let deadline = deadline(
                    timeout_ms,
                    if wait {
                        protocol::WAIT_TIMEOUT
                    } else {
                        protocol::SEND_TIMEOUT
                    },
                );
                match self.send(from, &to, &text, cx) {
                    Ok((found, info, envelope)) => {
                        let message =
                            self.message(found.id, info, envelope, wait, deadline, incoming, cx);
                        found
                            .session
                            .update(cx, |session, cx| session.queue_message(message, cx));
                        return;
                    }
                    Err(error) => Err(error),
                }
            }
            Request::Start {
                name,
                kind,
                tab,
                prompt,
                timeout_ms,
            } => match self.start_named(from, &name, kind, tab, cx) {
                Ok(sender) => {
                    self.starts.push(Starting {
                        name,
                        kind,
                        session: None,
                        prompt: prompt.map(|prompt| protocol::envelope(&sender, &prompt)),
                        deadline: deadline(timeout_ms, protocol::START_TIMEOUT),
                        reply: incoming,
                    });
                    return;
                }
                Err(error) => Err(error),
            },
        };
        incoming.reply(outcome(answer));
    }

    /// The workspace opened (or could not open) the pane of `hive agent start <name>`.
    pub(crate) fn started(&mut self, name: &str, session: Result<Entity<TerminalSession>, String>) {
        let Some(index) = self
            .starts
            .iter()
            .position(|start| start.name == name && start.session.is_none())
        else {
            return;
        };
        match session {
            Ok(session) => self.starts[index].session = Some(session.downgrade()),
            Err(error) => self
                .starts
                .remove(index)
                .reply
                .reply(Outcome::Error(ApiError::new(ErrorCode::AgentGone, error))),
        }
    }

    /// What a message delivery needs to know about the agent in `session` right now.
    pub(crate) fn recipient(&self, session: EntityId) -> Recipient {
        let watched = self.watched.iter().find(|watched| watched.id == session);
        let sighted = self.agent(session);
        let claude = sighted.is_some_and(|agent| agent.kind == AgentKind::ClaudeCode);
        let reported = watched
            .and_then(|watched| watched.reported)
            .filter(|_| claude);
        Recipient {
            status: reported.or(sighted.map(|agent| agent.status)),
            exact: reported.is_some(),
            awaiting_report: claude
                && reported.is_none()
                && self.hooks.is_some()
                && watched.is_some_and(|watched| {
                    watched.probe.launched_agent == Some(AgentKind::ClaudeCode)
                }),
        }
    }

    /// Hook reports arrive between polls; a turn that starts and ends within one poll still
    /// counts as seen working.
    pub(super) fn note_status(&mut self, session: EntityId, status: AgentStatus) {
        if status != AgentStatus::Working {
            return;
        }
        for wait in &mut self.waits {
            if wait.session == session {
                wait.seen_working = true;
            }
        }
    }

    /// Answers kept calls whose agent settled, appeared, vanished or ran out of time.
    pub(super) fn settle_calls(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        for mut wait in std::mem::take(&mut self.waits) {
            let alive = self.live_session(wait.session, cx).is_some();
            let status = self.agent(wait.session).map(|agent| agent.status);
            let verdict = settle(
                wait.after_submit,
                &mut wait.seen_working,
                status.filter(|_| alive),
                now >= wait.deadline,
            );
            match verdict {
                Verdict::Keep => self.waits.push(wait),
                Verdict::Settled => {
                    let info = self.info(wait.session);
                    wait.reply
                        .reply(outcome(info.map(|agent| Response::Agent { agent })));
                }
                Verdict::NoAgent => wait.reply.reply(Outcome::Error(ApiError::new(
                    ErrorCode::AgentGone,
                    "The agent exited or its pane closed",
                ))),
                Verdict::Expired => wait.reply.reply(Outcome::Error(ApiError::new(
                    ErrorCode::Timeout,
                    "The agent is still working",
                ))),
            }
        }
        for start in std::mem::take(&mut self.starts) {
            self.settle_start(start, now, cx);
        }
    }

    fn settle_start(&mut self, start: Starting, now: Instant, cx: &mut Context<Self>) {
        let failure =
            |message: String| Outcome::Error(ApiError::new(ErrorCode::AgentGone, message));
        let Some(weak) = start.session.clone() else {
            if now >= start.deadline {
                start.reply.reply(timed_out(&start.name));
            } else {
                self.starts.push(start);
            }
            return;
        };
        let Some(session) = weak.upgrade() else {
            start
                .reply
                .reply(failure(format!("The pane of {} closed", start.name)));
            return;
        };
        if let SessionStatus::Failed(error) = &session.read(cx).status {
            start
                .reply
                .reply(failure(format!("{} could not start: {error}", start.name)));
            return;
        }
        if session.read(cx).exited {
            start.reply.reply(failure(format!("{} exited", start.name)));
            return;
        }
        let id = session.entity_id();
        let info = self.info(id).ok().filter(|info| info.kind == start.kind);
        let Some(info) = info else {
            if now >= start.deadline {
                start.reply.reply(timed_out(&start.name));
            } else {
                self.starts.push(start);
            }
            return;
        };
        let Some(prompt) = start.prompt else {
            start
                .reply
                .reply(Outcome::Result(Response::Agent { agent: info }));
            return;
        };
        let name = start.name;
        let Message {
            agent,
            text,
            deadline,
            done,
        } = self.message(id, info, prompt, false, start.deadline, start.reply, cx);
        let done = Box::new(move |result: Result<(), ApiError>, cx: &mut App| {
            let result = result.map_err(|error| {
                ApiError::new(
                    error.code,
                    format!(
                        "{name} started but did not take its first task: {} Send it with hive agent send {name}.",
                        error.message
                    ),
                )
            });
            done(result, cx)
        });
        let message = Message {
            agent,
            text,
            deadline,
            done,
        };
        session.update(cx, |session, cx| session.queue_message(message, cx));
    }

    /// A message for the agent in `session` whose delivery answers `reply`. `info` is the
    /// agent as it was when the message was sent: a delivered message is always answered with
    /// success, also if the poll no longer sees the agent, so the sender never resends it.
    #[allow(clippy::too_many_arguments)]
    fn message(
        &self,
        session: EntityId,
        info: AgentInfo,
        text: String,
        wait: bool,
        deadline: Instant,
        reply: Incoming,
        cx: &mut Context<Self>,
    ) -> Message {
        let agents = cx.entity().downgrade();
        Message {
            agent: info.kind,
            text,
            deadline,
            done: Box::new(move |result, cx| {
                if let Err(error) = result {
                    reply.reply(Outcome::Error(error));
                    return;
                }
                let kept = agents.update(cx, |agents, _| {
                    if wait {
                        agents.waits.push(Waiting {
                            session,
                            after_submit: true,
                            seen_working: false,
                            deadline,
                            reply,
                        });
                        None
                    } else {
                        Some((agents.info(session).unwrap_or(info), reply))
                    }
                });
                if let Ok(Some((agent, reply))) = kept {
                    reply.reply(Outcome::Result(Response::Agent { agent }));
                }
            }),
        }
    }

    fn send(
        &self,
        from: Option<u64>,
        to: &Address,
        text: &str,
        cx: &App,
    ) -> Result<(Found, AgentInfo, String), ApiError> {
        let sender = self.sender(from, cx)?;
        let found = self.resolve(to, cx)?;
        if from == Some(found.id.as_u64()) {
            return Err(ApiError::new(
                ErrorCode::InvalidRequest,
                "An agent cannot send a message to itself",
            ));
        }
        let info = self.info(found.id).map_err(|_| no_agent(to))?;
        if self.recipient(found.id).status == Some(AgentStatus::WaitingApproval) {
            return Err(blocked(to));
        }
        let envelope = protocol::envelope(&sender, text);
        Ok((found, info, envelope))
    }

    /// Checks a `start` and asks the workspace to open the pane; returns the caller's label.
    fn start_named(
        &self,
        from: Option<u64>,
        name: &str,
        kind: AgentKind,
        tab: bool,
        cx: &mut Context<Self>,
    ) -> Result<Address, ApiError> {
        let sender = self.sender(from, cx)?;
        if !protocol::valid_name(name) {
            return Err(ApiError::new(
                ErrorCode::InvalidName,
                format!("{name:?} is not a valid agent name"),
            ));
        }
        if !AGENTS.contains(&kind) {
            return Err(ApiError::new(
                ErrorCode::Unsupported,
                format!("Hive cannot start {}", kind.name()),
            ));
        }
        if self.name_taken(name, cx) {
            return Err(ApiError::new(
                ErrorCode::NameTaken,
                format!("A pane is already named {name}; choose another name"),
            ));
        }
        let caller = self
            .watched
            .iter()
            .find(|watched| Some(watched.id.as_u64()) == from)
            .ok_or_else(caller_unknown)?;
        cx.emit(AgentsEvent::Spawn(SpawnRequest {
            caller: caller.id,
            host: caller.probe.host.clone(),
            worktree: caller.probe.worktree.clone(),
            kind,
            name: name.to_owned(),
            tab,
        }));
        Ok(sender)
    }

    /// The session to wait for, or `None` when its agent is not working and `wait` is
    /// answered at once.
    fn working_agent(&self, target: &Address, cx: &App) -> Result<Option<EntityId>, ApiError> {
        let found = self.resolve(target, cx)?;
        let agent = self.agent(found.id).ok_or_else(|| no_agent(target))?;
        Ok((agent.status == AgentStatus::Working).then_some(found.id))
    }

    fn read_terminal(
        &self,
        target: &Address,
        lines: Option<usize>,
        cx: &App,
    ) -> Result<Response, ApiError> {
        let found = self.resolve(target, cx)?;
        let lines = lines.unwrap_or(protocol::READ_LINES).min(MAX_READ_LINES);
        let text = found
            .session
            .read(cx)
            .terminal()
            .map(|terminal| terminal.last_lines(lines))
            .ok_or_else(|| {
                ApiError::new(ErrorCode::NotReady, format!("{target} has not started yet"))
            })?;
        Ok(Response::Text { text })
    }

    fn agent_response(&self, target: &Address, cx: &App) -> Result<Response, ApiError> {
        let found = self.resolve(target, cx)?;
        self.info(found.id).map(|agent| Response::Agent { agent })
    }

    /// The live local terminal `address` names.
    fn resolve(&self, address: &Address, cx: &App) -> Result<Found, ApiError> {
        let watched = self.watched.iter().find(|watched| match address {
            Address::Terminal(id) => watched.id.as_u64() == *id,
            Address::Name(name) => watched
                .session
                .upgrade()
                .is_some_and(|session| session.read(cx).name.as_deref() == Some(name.as_str())),
        });
        let watched = watched.ok_or_else(|| {
            ApiError::new(
                ErrorCode::UnknownTarget,
                format!("No open Hive pane is called {address}; run hive agent list"),
            )
        })?;
        if !watched.probe.host.is_local() {
            return Err(ApiError::new(
                ErrorCode::Unsupported,
                "Agents in SSH terminals cannot be reached yet",
            ));
        }
        let session = self
            .live_session(watched.id, cx)
            .ok_or_else(|| ApiError::new(ErrorCode::AgentGone, format!("{address} has exited")))?;
        Ok(Found {
            session,
            id: watched.id,
        })
    }

    fn live_session(&self, id: EntityId, cx: &App) -> Option<Entity<TerminalSession>> {
        self.watched
            .iter()
            .find(|watched| watched.id == id)
            .and_then(|watched| watched.session.upgrade())
            .filter(|session| !session.read(cx).exited)
    }

    /// How the caller signs its messages: its name, or its terminal id.
    fn sender(&self, from: Option<u64>, cx: &App) -> Result<Address, ApiError> {
        let id = from.ok_or_else(caller_unknown)?;
        let watched = self
            .watched
            .iter()
            .find(|watched| watched.id.as_u64() == id)
            .ok_or_else(caller_unknown)?;
        let name = watched
            .session
            .upgrade()
            .and_then(|session| session.read(cx).name.clone());
        Ok(name.map_or(Address::Terminal(id), Address::Name))
    }

    fn name_taken(&self, name: &str, cx: &App) -> bool {
        self.starts.iter().any(|start| start.name == name)
            || self.watched.iter().any(|watched| {
                watched
                    .session
                    .upgrade()
                    .is_some_and(|session| session.read(cx).name.as_deref() == Some(name))
            })
    }

    /// The agent the last poll saw in `session`.
    fn agent(&self, session: EntityId) -> Option<&AgentSession> {
        self.all()
            .map(|(_, agent)| agent)
            .find(|agent| agent.session == session)
    }

    fn info(&self, session: EntityId) -> Result<AgentInfo, ApiError> {
        self.all()
            .find(|(_, agent)| agent.session == session)
            .map(|((_, worktree), agent)| agent_info(worktree, agent))
            .ok_or_else(|| ApiError::new(ErrorCode::NoAgent, "The pane runs no agent now"))
    }

    fn infos(&self, cx: &App) -> Vec<AgentInfo> {
        // Terminal order, which is the order agents were opened in, stays stable across calls.
        self.watched
            .iter()
            .filter(|watched| watched.probe.host.is_local())
            .filter(|watched| self.live_session(watched.id, cx).is_some())
            .filter_map(|watched| self.info(watched.id).ok())
            .collect()
    }
}

fn agent_info(worktree: &Path, agent: &AgentSession) -> AgentInfo {
    AgentInfo {
        id: agent.session.as_u64(),
        name: agent.name.as_ref().map(ToString::to_string),
        kind: agent.kind,
        state: agent.status,
        worktree: worktree.to_owned(),
        title: agent.title.as_ref().map(ToString::to_string),
    }
}

/// Whether a kept `wait` is answered now.
fn settle(
    after_submit: bool,
    seen_working: &mut bool,
    status: Option<AgentStatus>,
    expired: bool,
) -> Verdict {
    match status {
        None => Verdict::NoAgent,
        Some(AgentStatus::Working) => {
            *seen_working = true;
            if expired {
                Verdict::Expired
            } else {
                Verdict::Keep
            }
        }
        Some(_) if !after_submit || *seen_working => Verdict::Settled,
        Some(_) if expired => Verdict::Expired,
        Some(_) => Verdict::Keep,
    }
}

fn deadline(timeout_ms: Option<u64>, default: Duration) -> Instant {
    Instant::now() + timeout_ms.map_or(default, Duration::from_millis)
}

fn outcome(answer: Result<Response, ApiError>) -> Outcome {
    match answer {
        Ok(response) => Outcome::Result(response),
        Err(error) => Outcome::Error(error),
    }
}

fn timed_out(name: &str) -> Outcome {
    Outcome::Error(ApiError::new(
        ErrorCode::Timeout,
        format!("{name} has not started in time; check its pane"),
    ))
}

fn caller_unknown() -> ApiError {
    ApiError::new(
        ErrorCode::InvalidRequest,
        "Run hive agent from a terminal of a Hive project",
    )
}

fn no_agent(target: &Address) -> ApiError {
    ApiError::new(ErrorCode::NoAgent, format!("{target} runs no agent now"))
}

fn blocked(target: &Address) -> ApiError {
    ApiError::new(
        ErrorCode::AgentBlocked,
        format!("{target} waits for a permission decision; ask the user to look at its pane"),
    )
}

#[cfg(test)]
mod tests {
    use super::{AgentStatus, Verdict, settle};

    #[test]
    fn plain_wait_answers_as_soon_as_the_agent_stops_working() {
        let mut seen = false;
        assert_eq!(
            settle(false, &mut seen, Some(AgentStatus::Idle), false),
            Verdict::Settled
        );
        assert_eq!(
            settle(false, &mut seen, Some(AgentStatus::Working), false),
            Verdict::Keep
        );
        assert_eq!(
            settle(false, &mut seen, Some(AgentStatus::WaitingApproval), false),
            Verdict::Settled
        );
    }

    #[test]
    fn wait_after_a_message_needs_the_turn_to_start_first() {
        let mut seen = false;
        assert_eq!(
            settle(true, &mut seen, Some(AgentStatus::Idle), false),
            Verdict::Keep
        );
        assert_eq!(
            settle(true, &mut seen, Some(AgentStatus::Working), false),
            Verdict::Keep
        );
        assert!(seen);
        assert_eq!(
            settle(true, &mut seen, Some(AgentStatus::Done), false),
            Verdict::Settled
        );
    }

    #[test]
    fn a_vanished_agent_or_a_passed_deadline_ends_the_wait() {
        let mut seen = false;
        assert_eq!(settle(false, &mut seen, None, false), Verdict::NoAgent);
        assert_eq!(
            settle(true, &mut seen, Some(AgentStatus::Idle), true),
            Verdict::Expired
        );
        assert_eq!(
            settle(false, &mut seen, Some(AgentStatus::Working), true),
            Verdict::Expired
        );
    }
}
