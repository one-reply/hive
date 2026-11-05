//! Retained agent drafts use the existing PTY, never shell commands or submit keys.
use super::*;
use hive_core::agent::{AgentKind, AgentStatus, ProcessTable};
use hive_core::terminal::TermMode;
use std::time::Instant;

const STARTUP_DEADLINE: Duration = Duration::from_secs(15);
const READINESS_POLL: Duration = Duration::from_millis(500);

#[derive(Clone)]
pub(crate) struct AgentDraft {
    pub(crate) agent: AgentKind,
    pub(crate) text: SharedString,
    pub(crate) status: DraftStatus,
}

#[derive(Clone)]
pub(crate) enum DraftStatus {
    Checking,
    Waiting(SharedString),
    Inserted,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Delivery {
    Automatic,
    Startup,
    Manual,
}

impl TerminalSession {
    pub(crate) fn prepare_agent_draft(
        &mut self,
        agent: AgentKind,
        text: String,
        starting: bool,
        cx: &mut Context<Self>,
    ) {
        if self
            .agent_draft
            .as_ref()
            .is_some_and(|draft| draft.agent == agent && draft.text.as_ref() == text)
        {
            return;
        }
        self.agent_draft = Some(AgentDraft {
            agent,
            text: text.into(),
            status: DraftStatus::Checking,
        });
        self.start_agent_draft(
            if starting {
                Delivery::Startup
            } else {
                Delivery::Automatic
            },
            cx,
        );
    }

    pub(crate) fn dismiss_agent_draft(&mut self, cx: &mut Context<Self>) {
        self.draft_task = None;
        self.agent_draft = None;
        cx.notify();
    }

    pub(crate) fn insert_agent_draft(&mut self, cx: &mut Context<Self>) {
        self.start_agent_draft(Delivery::Manual, cx);
    }

    fn start_agent_draft(&mut self, delivery: Delivery, cx: &mut Context<Self>) {
        let Some(draft) = self.agent_draft.as_mut() else {
            return;
        };
        draft.status = DraftStatus::Checking;
        let agent = draft.agent;
        self.draft_task = Some(cx.spawn(async move |this, cx| {
            let deadline = Instant::now() + STARTUP_DEADLINE;
            loop {
                let Ok((ready, failed)) = this.update(cx, |this, cx| (this.draft_ready(delivery, cx), matches!(this.status, SessionStatus::Failed(_)) || this.exited)) else { return };
                let result = match ready {
                    Ok(pid) => {
                        let foreground = cx.background_executor().spawn(async move {
                            ProcessTable::snapshot().map(|table| table.foreground_agent(pid))
                        }).await;
                        match foreground {
                            Ok(Some(kind)) if kind == agent => {
                                let Ok(result) = this.update(cx, |this, cx| {
                                    this.write_agent_draft(pid, delivery, cx)
                                }) else { return };
                                result
                            }
                            Ok(_) => Err("The selected agent is not in the foreground. Open its prompt, then choose Insert draft.".to_owned()),
                            Err(error) => {
                                tracing::warn!(%error, "could not verify draft recipient");
                                let _ = this.update(cx, |this, cx| this.finish_agent_draft(Err(error.to_string()), cx));
                                return;
                            }
                        }
                    }
                    Err(reason) => Err(reason),
                };
                if result.is_ok() || delivery != Delivery::Startup || failed || Instant::now() >= deadline {
                    let _ = this.update(cx, |this, cx| this.finish_agent_draft(result, cx));
                    return;
                }
                cx.background_executor().timer(READINESS_POLL).await;
            }
        }));
        cx.notify();
    }

    fn draft_ready(&self, delivery: Delivery, cx: &Context<Self>) -> Result<u32, String> {
        if let SessionStatus::Failed(error) = &self.status {
            return Err(format!(
                "Could not start the agent: {error}. The draft is kept here."
            ));
        }
        let Some(terminal) = self.terminal().filter(|_| !self.exited) else {
            return Err("Waiting for the agent to start. The draft is kept here.".into());
        };
        if self.paste_task.is_some() || !self.pending_input.is_empty() {
            return Err("Another paste is pending. Try Insert draft when it finishes.".into());
        }
        if !terminal.mode().contains(TermMode::BRACKETED_PASTE) {
            return Err("Open the agent's message field, then choose Insert draft.".into());
        }
        let status = AppState::get(cx)
            .agents
            .read(cx)
            .all()
            .find(|(_, session)| session.session == cx.entity_id())
            .map(|(_, session)| session.status);
        if !can_insert(
            delivery,
            status,
            terminal.activity().is_streaming(Instant::now()),
        ) {
            return Err("The agent may be busy or waiting for input. Open its message field, then choose Insert draft.".into());
        }
        Ok(terminal.shell_pid())
    }

    fn write_agent_draft(
        &mut self,
        pid: u32,
        delivery: Delivery,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.draft_ready(delivery, cx)? != pid {
            return Err("The terminal changed. Choose Send to agent again.".into());
        }
        let draft = self.agent_draft.as_ref().ok_or("The draft was dismissed")?;
        let terminal = self.terminal().ok_or("The terminal closed")?;
        let bytes = hive_core::agent::draft::paste_bytes(&draft.text, terminal.mode())
            .ok_or("The message field is not ready for a multiline paste")?;
        self.enqueue_input(Input::Bytes(bytes), cx);
        Ok(())
    }

    fn finish_agent_draft(&mut self, result: Result<(), String>, cx: &mut Context<Self>) {
        self.draft_task = None;
        if let Some(draft) = &mut self.agent_draft {
            draft.status = match result {
                Ok(()) => DraftStatus::Inserted,
                Err(reason) => DraftStatus::Waiting(reason.into()),
            };
            cx.notify();
        }
    }
}

fn can_insert(delivery: Delivery, status: Option<AgentStatus>, streaming: bool) -> bool {
    // shortcut: Idle is estimated for CLIs without hooks; keep a visible backup for retry.
    delivery == Delivery::Manual
        || (!streaming
            && matches!(
                status,
                Some(
                    AgentStatus::Idle
                        | AgentStatus::WaitingInput
                        | AgentStatus::Done
                        | AgentStatus::Failed
                )
            ))
}

#[cfg(test)]
mod tests {
    use super::{AgentStatus, Delivery, can_insert};
    #[test]
    fn automatic_paste_waits_for_idle_and_never_answers_approval() {
        for status in [
            None,
            Some(AgentStatus::Working),
            Some(AgentStatus::WaitingApproval),
        ] {
            assert!(!can_insert(Delivery::Automatic, status, false));
        }
        assert!(!can_insert(
            Delivery::Automatic,
            Some(AgentStatus::Idle),
            true
        ));
        assert!(can_insert(
            Delivery::Automatic,
            Some(AgentStatus::Idle),
            false
        ));
        assert!(can_insert(
            Delivery::Automatic,
            Some(AgentStatus::Done),
            false
        ));
        assert!(can_insert(
            Delivery::Manual,
            Some(AgentStatus::Working),
            true
        ));
    }
}
