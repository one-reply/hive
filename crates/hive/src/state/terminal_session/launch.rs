//! Decide where a menu launch belongs using a fresh process snapshot, off the UI thread.
use super::{SessionEvent, SessionStatus, TerminalSession};
use gpui_kit::Context;
use hive_core::agent::{AgentKind, ProcessTable};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AgentPlacement {
    CurrentPane,
    Below,
}

impl TerminalSession {
    pub(crate) fn request_agent(&mut self, agent: AgentKind, cx: &mut Context<Self>) {
        if !self.host.is_local() || self.worktree.is_none() || self.exited {
            return;
        }
        self.launch_task = None;
        if let Some(placement) = startup_placement(&self.status, self.launched_agent) {
            cx.emit(SessionEvent::StartAgent(agent, placement));
            return;
        }
        let Some(terminal) = self.terminal() else {
            return;
        };
        // Pending pastes must finish before a launch can hide this shell.
        if self.paste_task.is_some() || !self.pending_input.is_empty() {
            cx.emit(SessionEvent::StartAgent(agent, AgentPlacement::Below));
            return;
        }
        let pid = terminal.shell_pid();
        self.launch_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { ProcessTable::snapshot().map(|table| placement(&table, pid)) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.launch_task = None;
                if this.exited
                    || this
                        .terminal()
                        .is_none_or(|terminal| terminal.shell_pid() != pid)
                {
                    return;
                }
                match result {
                    Ok(placement) => cx.emit(SessionEvent::StartAgent(agent, placement)),
                    Err(error) => {
                        tracing::warn!(%error, "could not check terminal before agent launch");
                        cx.emit(SessionEvent::LaunchFailed(error.to_string()));
                    }
                }
            });
        }));
    }
}

fn startup_placement(
    status: &SessionStatus,
    launched: Option<AgentKind>,
) -> Option<AgentPlacement> {
    match (status, launched) {
        (SessionStatus::Failed(_), _) | (SessionStatus::Starting, None) => {
            Some(AgentPlacement::CurrentPane)
        }
        (_, Some(_)) => Some(AgentPlacement::Below),
        (SessionStatus::Ready(_), None) => None,
    }
}

fn placement(table: &ProcessTable, pid: u32) -> AgentPlacement {
    // macOS login owns the PTY; the foreground shell usually has a different PID.
    if table.foreground_agent(pid).is_none() && !table.foreground_pids(pid).is_empty() {
        AgentPlacement::CurrentPane
    } else {
        AgentPlacement::Below
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AgentKind, AgentPlacement, ProcessTable, SessionStatus, placement, startup_placement,
    };

    // Captured ps shapes from hive_core::agent::tests and the macOS login-shell smoke test.
    const PROCESSES: &str = "52000 52000 52000 -zsh
51000 51000 98260 -zsh
98260 98260 98260 claude
53000 53000 53100 -zsh
53100 53100 53100 vim claude.md
56100 56100 56100 /Users/me/.nvm/versions/node/v24.14.0/bin/node /Users/me/.nvm/versions/node/v24.14.0/bin/claude
43889 43889 43892 /usr/bin/login -qflp user /bin/zsh -fc exec -a -zsh /bin/zsh
43892 43892 43892 -zsh
";

    #[test]
    fn starting_agents_are_occupied_but_starting_shells_and_failed_launches_are_reusable() {
        assert_eq!(
            startup_placement(&SessionStatus::Starting, Some(AgentKind::Codex)),
            Some(AgentPlacement::Below)
        );
        assert_eq!(
            startup_placement(&SessionStatus::Starting, None),
            Some(AgentPlacement::CurrentPane)
        );
        assert_eq!(
            startup_placement(
                &SessionStatus::Failed("missing CLI".into()),
                Some(AgentKind::Codex)
            ),
            Some(AgentPlacement::CurrentPane)
        );
    }

    #[test]
    fn non_agent_sessions_reuse_their_pane_including_login_wrapped_shells() {
        for pid in [52000, 43889, 53000] {
            assert_eq!(
                placement(&ProcessTable::parse(PROCESSES), pid),
                AgentPlacement::CurrentPane
            );
        }
    }

    #[test]
    fn running_agents_open_below_including_direct_launches() {
        for pid in [51000, 56100] {
            assert_eq!(
                placement(&ProcessTable::parse(PROCESSES), pid),
                AgentPlacement::Below
            );
        }
    }

    #[test]
    fn missing_or_malformed_process_data_never_reuses_a_pane() {
        for sample in [PROCESSES, "", "malformed", "52000 52000 0 -zsh"] {
            assert_eq!(
                placement(&ProcessTable::parse(sample), 99999),
                AgentPlacement::Below
            );
        }
        assert_eq!(
            placement(&ProcessTable::parse("52000 52000 0 -zsh"), 52000),
            AgentPlacement::Below
        );
    }
}
