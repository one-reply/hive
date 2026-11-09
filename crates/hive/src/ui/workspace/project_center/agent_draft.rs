//! Reuse the matching agent or open a right-hand pane in the requested worktree.
use super::*;
use crate::state::terminal_session::SessionStatus;
use hive_core::agent::{AgentKind, launch::Launch};

impl ProjectCenter {
    pub(in crate::ui::workspace) fn prepare_agent_draft(
        &mut self,
        worktree: &Path,
        agent: AgentKind,
        prompt: String,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.prune(cx);
        let existing = AppState::get(cx)
            .agents
            .read(cx)
            .preferred_session(&self.host, worktree, agent, cx);
        if let Some(session) = existing {
            self.reveal_session(session.entity_id(), window, cx);
            session.update(cx, |session, cx| {
                session.prepare_agent_draft(agent, prompt, false, cx)
            });
            return;
        }
        let panes: Vec<_> = self
            .terminals
            .iter()
            .flat_map(|panel| {
                panel
                    .read(cx)
                    .worktree_panes(worktree, cx)
                    .into_iter()
                    .map(|pane| (panel.clone(), pane))
            })
            .collect();
        // A directly launched CLI may not appear in the next process poll yet.
        let starting = panes.iter().find_map(|(_, pane)| {
            let session = pane.read(cx).session();
            let state = session.read(cx);
            (state.launched_agent == Some(agent)
                && state.agent_draft.is_some()
                && !matches!(state.status, SessionStatus::Failed(_)))
            .then(|| session.clone())
        });
        if let Some(session) = starting {
            self.reveal_session(session.entity_id(), window, cx);
            session.update(cx, |session, cx| {
                session.prepare_agent_draft(agent, prompt, true, cx)
            });
            return;
        }
        let recent = panes
            .into_iter()
            .enumerate()
            .max_by_key(|(index, (_, pane))| {
                (
                    pane.read(cx).session().read(cx).last_focused,
                    std::cmp::Reverse(*index),
                )
            })
            .map(|(_, pane)| pane);
        let session = if let Some((panel, pane)) = recent {
            self.select(&panel, window, cx);
            panel.update(cx, |panel, cx| {
                panel.split_agent(
                    pane,
                    worktree,
                    Launch::New(agent),
                    Axis::Horizontal,
                    window,
                    cx,
                )
            })
        } else {
            let panel = TerminalPanel::open_with_launch(
                &self.host,
                Some(worktree),
                Some(Launch::New(agent)),
                self.dock.downgrade(),
                window,
                cx,
            );
            let session = panel.read(cx).active_session(cx);
            self.add(panel_handle(panel.clone()), window, cx);
            self.terminals.push(panel);
            self.sync_alone(cx);
            session
        };
        session.update(cx, |session, cx| {
            session.prepare_agent_draft(agent, prompt, true, cx)
        });
    }
}
