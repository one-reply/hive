//! Closing a pane or tab in which a CLI agent runs asks first, unless the user turned that off
//! in Settings → Terminal. A shell that exits on its own closes without asking.
use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::button::ButtonVariant;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::{WindowExt as _, v_flex};

use super::*;
use crate::state::agents::AgentSession;

/// What the user asked to close.
#[derive(Clone)]
enum Closing {
    Pane(Entity<TerminalView>),
    Tab,
}

impl TerminalPanel {
    pub(super) fn request_close_pane(
        &mut self,
        view: Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.request_close(Closing::Pane(view), window, cx);
    }

    pub(crate) fn request_close_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.request_close(Closing::Tab, window, cx);
    }

    fn request_close(&mut self, closing: Closing, window: &mut Window, cx: &mut Context<Self>) {
        let store = AppState::get(cx).store.clone();
        let agents = if store.read(cx).state.settings.confirm_close_running_agent {
            self.running_agents(&closing, cx)
        } else {
            Vec::new()
        };
        if agents.is_empty() {
            self.finish_close(&closing, window, cx);
            return;
        }
        let panel = cx.entity().downgrade();
        let (title, body) = prompt_text(agents.len(), matches!(closing, Closing::Tab));
        let statuses: Vec<SharedString> = agents
            .iter()
            .map(|agent| {
                format!(
                    "{} · {}",
                    agent.kind.name(),
                    if self.host.is_local() {
                        agent.status.label()
                    } else {
                        "SSH"
                    }
                )
                .into()
            })
            .collect();
        let dont_ask = Rc::new(Cell::new(false));
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (panel, closing, store) = (panel.clone(), closing.clone(), store.clone());
            let checkbox_state = dont_ask.clone();
            let ok_state = dont_ask.clone();
            alert
                .confirm()
                .title(title.clone())
                .description(
                    v_flex()
                        .gap_3()
                        .child(body.clone())
                        .children(statuses.iter().cloned())
                        .child(
                            Checkbox::new("dont-ask-close-running-agent")
                                .label("Don't ask again for running agents")
                                .checked(dont_ask.get())
                                .on_click(move |checked, window, _| {
                                    checkbox_state.set(*checked);
                                    window.refresh();
                                }),
                        ),
                )
                .ok_text(if statuses.len() == 1 {
                    "Stop Agent"
                } else {
                    "Stop Agents"
                })
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    if ok_state.get() {
                        store.update(cx, |store, cx| {
                            store.set_confirm_close_running_agent(false, cx)
                        });
                    }
                    if let Some(panel) = panel.upgrade() {
                        panel.update(cx, |panel, cx| panel.finish_close(&closing, window, cx));
                    }
                    true
                })
        });
    }

    fn finish_close(&mut self, closing: &Closing, window: &mut Window, cx: &mut Context<Self>) {
        match closing {
            Closing::Pane(view) => self.close_pane(view, window, cx),
            Closing::Tab => self.close(window, cx),
        }
    }

    /// Agents the last poll saw in the terminals this close would end, in pane order.
    fn running_agents(&self, closing: &Closing, cx: &App) -> Vec<AgentSession> {
        let mut sessions = Vec::new();
        match closing {
            Closing::Pane(view) => sessions.push(view.read(cx).session().entity_id()),
            Closing::Tab => self
                .panes
                .for_each(&mut |view| sessions.push(view.read(cx).session().entity_id())),
        }
        let agents = AppState::get(cx).agents.read(cx);
        let agents: Vec<_> = agents.all().map(|(_, agent)| agent).collect();
        agents_in(&sessions, &agents)
    }
}

/// The agents running in `sessions`, ordered like `sessions`.
fn agents_in(sessions: &[EntityId], agents: &[&AgentSession]) -> Vec<AgentSession> {
    sessions
        .iter()
        .filter_map(|session| agents.iter().find(|agent| agent.session == *session))
        .map(|agent| (*agent).clone())
        .collect()
}

/// Title and explanation of the confirmation for `count` agents in a pane or a whole tab.
fn prompt_text(count: usize, tab: bool) -> (SharedString, SharedString) {
    let target = if tab { "tab" } else { "pane" };
    if count == 1 {
        return (
            "Stop this agent?".into(),
            format!("Closing this terminal {target} stops the agent's current work.").into(),
        );
    }
    (
        format!("Stop {count} agents?").into(),
        format!("Closing this terminal {target} stops the current work of every agent in it.")
            .into(),
    )
}

#[cfg(test)]
mod tests {
    // Not a glob import: it would bring in GPUI's `test` attribute in place of std's.
    use super::{AgentSession, agents_in, prompt_text};
    use gpui_kit::EntityId;
    use hive_core::agent::{AgentKind, AgentStatus};

    fn agent(session: u64, kind: AgentKind) -> AgentSession {
        AgentSession {
            session: EntityId::from(session),
            kind,
            status: AgentStatus::Working,
            reported: false,
            unread: false,
            title: None,
            name: None,
        }
    }

    #[test]
    fn only_agents_of_closed_sessions_block_in_pane_order() {
        let agents = [
            agent(1, AgentKind::Codex),
            agent(2, AgentKind::ClaudeCode),
            agent(3, AgentKind::Codex),
        ];
        let sessions = [EntityId::from(3), EntityId::from(4), EntityId::from(1)];
        let found = agents_in(&sessions, &agents.iter().collect::<Vec<_>>());
        let ids: Vec<_> = found.iter().map(|agent| agent.session).collect();
        assert_eq!(ids, [EntityId::from(3), EntityId::from(1)]);
    }

    #[test]
    fn plain_shells_do_not_block() {
        let agents = [agent(1, AgentKind::Codex)];
        assert!(agents_in(&[EntityId::from(2)], &[&agents[0]]).is_empty());
        assert!(agents_in(&[], &[&agents[0]]).is_empty());
    }

    #[test]
    fn prompt_names_scope_and_count() {
        let (title, body) = prompt_text(1, false);
        assert_eq!(title.as_ref(), "Stop this agent?");
        assert!(body.contains("terminal pane"));
        let (title, body) = prompt_text(3, true);
        assert_eq!(title.as_ref(), "Stop 3 agents?");
        assert!(body.contains("terminal tab"));
    }
}
