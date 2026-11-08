//! Messages from other agents (`hive agent send`): pasted into this terminal's agent prompt and
//! submitted with Enter. Enter answers whatever the agent shows, so delivery makes sure the
//! paste lands in the prompt and not in a dialog, where it would decide for the user.
//!
//! Claude Code reports permission prompts (through hooks or OSC 7501) as it shows them and
//! queues input typed during a turn, so it gets messages at once, even while it works. Other
//! agents that report their state get a message once they stop working; agents that report
//! nothing once their output pauses. Either way the message is submitted only if the paste
//! visibly changed the screen.
use super::*;
use hive_core::agent::api::{ApiError, ErrorCode};
use hive_core::agent::{AgentKind, AgentStatus, ProcessTable};
use hive_core::terminal::TermMode;
use std::time::Instant;

/// How often a message that cannot be delivered yet is retried until its deadline.
const RETRY: Duration = Duration::from_millis(250);
/// Time for the agent's prompt to draw the paste before Enter is pressed.
const ECHO_DELAY: Duration = Duration::from_millis(200);
/// Keystrokes this recent mean the user is typing into the same prompt.
const TYPING_WINDOW: Duration = Duration::from_secs(2);

/// Called once with the delivery result, outside any entity update.
pub(crate) type Delivered = Box<dyn FnOnce(Result<(), ApiError>, &mut App)>;

/// A message waiting for this terminal's agent.
pub(crate) struct Message {
    /// The agent the sender addressed; a different foreground program gets nothing.
    pub(crate) agent: AgentKind,
    /// The text as the recipient will see it, sender and reply hint included.
    pub(crate) text: String,
    pub(crate) deadline: Instant,
    pub(crate) done: Delivered,
}

/// What Hive knows about the recipient, from [`crate::state::agents::Agents::recipient`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Recipient {
    pub(crate) status: Option<AgentStatus>,
    /// Reported by the agent itself (hooks, OSC 7501) rather than estimated from output.
    pub(crate) exact: bool,
    /// The agent takes typed input during a turn and runs it next, as Claude Code does.
    pub(crate) queues_input: bool,
    /// Claude Code started by Hive that has not reported yet: it may show a trust prompt.
    pub(crate) awaiting_report: bool,
}

/// The next delivery step the terminal allows.
#[derive(Debug, PartialEq)]
enum Step {
    /// Paste now; with `verify` the screen must change before Enter is pressed.
    Paste {
        verify: bool,
    },
    Retry(&'static str),
    Refuse(ErrorCode, &'static str),
}

impl TerminalSession {
    /// `hive agent interrupt`: Esc stops the agent's turn. The caller made sure it shows no
    /// dialog, where Esc would answer for the user.
    pub(crate) fn interrupt(&mut self, cx: &mut Context<Self>) {
        self.enqueue_input(Input::Bytes(vec![0x1b]), cx);
    }

    pub(crate) fn queue_message(&mut self, message: Message, cx: &mut Context<Self>) {
        self.inbox.push_back(message);
        if self.inbox_task.is_none() {
            self.deliver_inbox(cx);
        }
    }

    /// Answers every queued message: the terminal is gone.
    pub(super) fn fail_inbox(&mut self, cx: &mut Context<Self>) {
        self.inbox_task = None;
        let messages: Vec<_> = self.inbox.drain(..).collect();
        if messages.is_empty() {
            return;
        }
        // Answering may update `Agents`; never from inside this entity's update.
        cx.defer(move |cx| {
            for message in messages {
                (message.done)(
                    Err(ApiError::new(
                        ErrorCode::AgentGone,
                        "The recipient's terminal exited",
                    )),
                    cx,
                );
            }
        });
    }

    fn deliver_inbox(&mut self, cx: &mut Context<Self>) {
        self.inbox_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let next = this.update(cx, |this, _| {
                    let next = this
                        .inbox
                        .front()
                        .map(|message| (message.agent, message.deadline));
                    if next.is_none() {
                        this.inbox_task = None;
                    }
                    next
                });
                let Ok(Some((agent, deadline))) = next else {
                    return;
                };
                let result = deliver(&this, agent, deadline, cx).await;
                let Ok(Some(message)) = this.update(cx, |this, _| this.inbox.pop_front()) else {
                    return;
                };
                if result.is_err() {
                    tracing::info!(terminal = ?this.entity_id(), "agent message not delivered");
                }
                cx.update(|cx| (message.done)(result, cx));
            }
        }));
    }

    fn message_step(&self, agent: AgentKind, cx: &Context<Self>) -> Result<(Step, u32), ApiError> {
        if let SessionStatus::Failed(error) = &self.status {
            return Err(ApiError::new(ErrorCode::AgentGone, error.clone()));
        }
        if self.exited {
            return Err(ApiError::new(ErrorCode::AgentGone, "The terminal exited"));
        }
        let Some(terminal) = self.terminal() else {
            return Ok((Step::Retry("The terminal is still starting"), 0));
        };
        let pid = terminal.shell_pid();
        if self.paste_task.is_some() || !self.pending_input.is_empty() {
            return Ok((
                Step::Retry("Another paste into this terminal is pending"),
                pid,
            ));
        }
        if !terminal.mode().contains(TermMode::BRACKETED_PASTE) {
            return Ok((Step::Retry("The agent's prompt is not open"), pid));
        }
        let recipient = AppState::get(cx).agents.read(cx).recipient(cx.entity_id());
        let now = Instant::now();
        let activity = terminal.activity();
        let step = step(
            recipient,
            activity.is_streaming(now),
            activity.had_input_within(TYPING_WINDOW, now),
        );
        tracing::trace!(?agent, ?step, "agent message readiness");
        Ok((step, pid))
    }

    /// Pastes the next message if the terminal still allows it; returns the screen before the
    /// paste when Enter must wait for it to change, `Ok(None)` to retry later.
    fn paste_message(
        &mut self,
        agent: AgentKind,
        pid: u32,
        cx: &mut Context<Self>,
    ) -> Result<Option<Option<String>>, ApiError> {
        let (step, current) = self.message_step(agent, cx)?;
        let verify = match step {
            Step::Paste { verify } if current == pid => verify,
            Step::Refuse(code, reason) => return Err(ApiError::new(code, reason)),
            _ => return Ok(None),
        };
        let terminal = self.terminal().ok_or_else(gone)?;
        let text = &self.inbox.front().ok_or_else(gone)?.text;
        let bytes = hive_core::agent::draft::paste_bytes(text, terminal.mode())
            .ok_or_else(|| ApiError::new(ErrorCode::NotReady, "The agent's prompt is not open"))?;
        let before = verify.then(|| screen(terminal));
        self.enqueue_input(Input::Bytes(bytes), cx);
        Ok(Some(before))
    }

    fn submit_message(
        &mut self,
        before: Option<String>,
        cx: &mut Context<Self>,
    ) -> Result<(), ApiError> {
        let terminal = self.terminal().filter(|_| !self.exited).ok_or_else(gone)?;
        let recipient = AppState::get(cx).agents.read(cx).recipient(cx.entity_id());
        if recipient.status == Some(AgentStatus::WaitingApproval) {
            return Err(ApiError::new(
                ErrorCode::AgentBlocked,
                "A permission prompt appeared after the message was pasted; it was not submitted and stays in the agent's prompt",
            ));
        }
        if before.is_some_and(|before| before == screen(terminal)) {
            return Err(ApiError::new(
                ErrorCode::NotReady,
                "The agent did not show the pasted message, so it may be displaying a dialog; nothing was submitted",
            ));
        }
        self.enqueue_input(Input::Bytes(b"\r".to_vec()), cx);
        Ok(())
    }
}

/// Retries until `deadline`; every step re-reads the terminal and the freshest hook reports.
async fn deliver(
    this: &WeakEntity<TerminalSession>,
    agent: AgentKind,
    deadline: Instant,
    cx: &mut AsyncApp,
) -> Result<(), ApiError> {
    let agents = cx.update(|cx| AppState::get(cx).agents.clone());
    let refresh = |cx: &mut AsyncApp| {
        agents.update(cx, |agents, cx| agents.refresh_reports(cx));
    };
    loop {
        refresh(cx);
        let (step, pid) = this
            .update(cx, |this, cx| this.message_step(agent, cx))
            .map_err(|_| gone())??;
        let reason = match step {
            Step::Refuse(code, reason) => return Err(ApiError::new(code, reason)),
            Step::Retry(reason) => reason,
            Step::Paste { .. } => {
                let foreground = cx
                    .background_executor()
                    .spawn(async move {
                        ProcessTable::snapshot().map(|table| table.foreground_agent(pid))
                    })
                    .await;
                match foreground {
                    Ok(Some(kind)) if kind == agent => {}
                    Ok(_) => {
                        return Err(ApiError::new(
                            ErrorCode::NoAgent,
                            format!(
                                "{} is no longer in the foreground of its pane",
                                agent.name()
                            ),
                        ));
                    }
                    Err(error) => {
                        return Err(ApiError::new(ErrorCode::NotReady, error.to_string()));
                    }
                }
                refresh(cx);
                let pasted = this
                    .update(cx, |this, cx| this.paste_message(agent, pid, cx))
                    .map_err(|_| gone())??;
                if let Some(before) = pasted {
                    cx.background_executor().timer(ECHO_DELAY).await;
                    refresh(cx);
                    return this
                        .update(cx, |this, cx| this.submit_message(before, cx))
                        .map_err(|_| gone())?;
                }
                "The agent became busy"
            }
        };
        if Instant::now() >= deadline {
            return Err(ApiError::new(ErrorCode::NotReady, reason));
        }
        cx.background_executor().timer(RETRY).await;
    }
}

/// Exact status lets a busy Claude Code queue the message; anything else waits until the agent
/// stops working (reported, or its output paused) and proves the paste reached a prompt.
fn step(recipient: Recipient, streaming: bool, typing: bool) -> Step {
    match recipient.status {
        Some(AgentStatus::WaitingApproval) => {
            return Step::Refuse(
                ErrorCode::AgentBlocked,
                "The agent waits for a permission decision; ask the user to look at its pane",
            );
        }
        None => return Step::Retry("The agent has not appeared in its pane yet"),
        Some(_) => {}
    }
    if recipient.awaiting_report {
        return Step::Retry(
            "Claude Code has not finished starting; it may be waiting for the user, such as a trust prompt",
        );
    }
    if typing {
        return Step::Retry("The user is typing in this pane");
    }
    // WaitingInput also covers Claude's question dialogs, which hooks do not tell apart.
    if recipient.exact
        && recipient.queues_input
        && recipient.status != Some(AgentStatus::WaitingInput)
    {
        return Step::Paste { verify: false };
    }
    if recipient.exact && recipient.status == Some(AgentStatus::Working) {
        return Step::Retry("The agent is working");
    }
    if streaming {
        return Step::Retry("The agent is busy; its output has not paused");
    }
    Step::Paste { verify: true }
}

fn screen(terminal: &Terminal) -> String {
    terminal.last_lines(usize::from(terminal.size().rows))
}

fn gone() -> ApiError {
    ApiError::new(ErrorCode::AgentGone, "The recipient's pane closed")
}

#[cfg(test)]
mod tests {
    use super::{AgentStatus, ErrorCode, Recipient, Step, step};

    fn recipient(status: AgentStatus, exact: bool) -> Recipient {
        Recipient {
            status: Some(status),
            exact,
            queues_input: exact,
            awaiting_report: false,
        }
    }

    #[test]
    fn permission_prompts_are_never_answered() {
        for exact in [true, false] {
            assert!(matches!(
                step(recipient(AgentStatus::WaitingApproval, exact), false, false),
                Step::Refuse(ErrorCode::AgentBlocked, _)
            ));
        }
    }

    #[test]
    fn claude_with_hooks_takes_messages_even_while_working() {
        assert_eq!(
            step(recipient(AgentStatus::Working, true), true, false),
            Step::Paste { verify: false }
        );
        assert_eq!(
            step(recipient(AgentStatus::Done, true), false, false),
            Step::Paste { verify: false }
        );
    }

    #[test]
    fn estimated_agents_wait_for_a_pause_and_verify_the_paste() {
        assert!(matches!(
            step(recipient(AgentStatus::Working, false), true, false),
            Step::Retry(_)
        ));
        assert_eq!(
            step(recipient(AgentStatus::Idle, false), false, false),
            Step::Paste { verify: true }
        );
        assert_eq!(
            step(recipient(AgentStatus::WaitingInput, true), false, false),
            Step::Paste { verify: true }
        );
    }

    #[test]
    fn exact_agents_that_do_not_queue_input_wait_for_the_turn_and_verify_the_paste() {
        let codex = |status| Recipient {
            queues_input: false,
            ..recipient(status, true)
        };
        assert!(matches!(
            step(codex(AgentStatus::Working), false, false),
            Step::Retry(_)
        ));
        assert_eq!(
            step(codex(AgentStatus::Idle), false, false),
            Step::Paste { verify: true }
        );
        // A reported Idle with output still streaming is not a pause yet.
        assert!(matches!(
            step(codex(AgentStatus::Idle), true, false),
            Step::Retry(_)
        ));
        assert!(matches!(
            step(codex(AgentStatus::WaitingApproval), false, false),
            Step::Refuse(ErrorCode::AgentBlocked, _)
        ));
    }

    #[test]
    fn startup_and_typing_users_hold_messages_back() {
        let starting = Recipient {
            awaiting_report: true,
            ..recipient(AgentStatus::Idle, false)
        };
        assert!(matches!(step(starting, false, false), Step::Retry(_)));
        assert!(matches!(
            step(recipient(AgentStatus::Idle, true), false, true),
            Step::Retry(_)
        ));
        let unseen = Recipient {
            status: None,
            exact: false,
            queues_input: false,
            awaiting_report: false,
        };
        assert!(matches!(step(unseen, false, false), Step::Retry(_)));
    }
}
