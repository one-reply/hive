//! Ending a terminal in which a CLI agent runs asks first, unless the user turned that off in
//! Settings → Terminal: closing its pane or tab, closing the window, quitting Hive. A shell
//! that exits on its own closes without asking.
//!
//! "An agent runs" has one meaning for all of them: the last poll of `Agents` saw one in the
//! terminal. What differs is which terminals an action ends.
use std::cell::Cell;
use std::rc::Rc;

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

/// What is about to end the agents' terminals; it words the question.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ending {
    Pane,
    Tab,
    /// The main window: every terminal in it.
    Window,
    /// Hive itself: every terminal it runs, also for an update that restarts it.
    App,
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
        let ending = match closing {
            Closing::Pane(_) => Ending::Pane,
            Closing::Tab => Ending::Tab,
        };
        let agents = self.agent_lines(&closing, cx);
        let panel = cx.entity().downgrade();
        confirm_stopping(ending, agents, window, cx, move |window, cx| {
            if let Some(panel) = panel.upgrade() {
                panel.update(cx, |panel, cx| panel.finish_close(&closing, window, cx));
            }
        });
    }

    fn finish_close(&mut self, closing: &Closing, window: &mut Window, cx: &mut Context<Self>) {
        match closing {
            Closing::Pane(view) => self.close_pane(view, Reveal::Requested, window, cx),
            Closing::Tab => self.close(Reveal::Requested, window, cx),
        }
    }

    /// The agents running in this tab, one line each, as the question about stopping them
    /// lists them. The window asks every tab before it closes.
    pub(crate) fn running_agents(&self, cx: &App) -> Vec<SharedString> {
        self.agent_lines(&Closing::Tab, cx)
    }

    /// Agents the last poll saw in the terminals this close would end, in pane order.
    fn agent_lines(&self, closing: &Closing, cx: &App) -> Vec<SharedString> {
        let mut sessions = Vec::new();
        match closing {
            Closing::Pane(view) => sessions.push(view.read(cx).session().entity_id()),
            Closing::Tab => self
                .panes
                .for_each(&mut |view| sessions.push(view.read(cx).session().entity_id())),
        }
        let agents = AppState::get(cx).agents.read(cx);
        let agents: Vec<_> = agents.all().map(|(_, agent)| agent).collect();
        let local = self.host.is_local();
        agents_in(&sessions, &agents)
            .iter()
            .map(|agent| agent_line(agent, local))
            .collect()
    }
}

/// Every agent the last poll saw, in any terminal Hive runs: what quitting stops. Unlike a
/// closing window, that includes the terminals of pipeline steps, which no tab holds.
pub(crate) fn all_running_agents(cx: &App) -> Vec<SharedString> {
    AppState::get(cx)
        .agents
        .read(cx)
        .all()
        .map(|((host, _), agent)| agent_line(agent, host.is_local()))
        .collect()
}

/// Whether ending `agents` is asked about at all: there is one, and the user has not turned
/// the question off.
pub(crate) fn asks_before_stopping(agents: &[SharedString], cx: &App) -> bool {
    !agents.is_empty()
        && AppState::get(cx)
            .store
            .read(cx)
            .state
            .settings
            .confirm_close_running_agent
}

/// Runs `proceed` once the user agrees to stop `agents`, or once the caller's update is over
/// when there is nothing to ask. A dismissed question drops `proceed` uncalled, which is how a
/// quit learns "no".
///
/// Never on the caller's stack: a tab's close button asks from inside the panel's update, and
/// `proceed` closes the panel, which would update it a second time.
pub(crate) fn confirm_stopping(
    ending: Ending,
    agents: Vec<SharedString>,
    window: &mut Window,
    cx: &mut App,
    proceed: impl FnOnce(&mut Window, &mut App) + 'static,
) {
    if !asks_before_stopping(&agents, cx) {
        window.defer(cx, proceed);
        return;
    }
    let store = AppState::get(cx).store.clone();
    ask(
        ending,
        agents,
        window,
        cx,
        move |dont_ask_again, window, cx| {
            if dont_ask_again {
                store.update(cx, |store, cx| {
                    store.set_confirm_close_running_agent(false, cx)
                });
            }
            proceed(window, cx);
        },
    );
}

/// What runs on the answer "yes", told whether "Don't ask again" was ticked.
type Answer = Box<dyn FnOnce(bool, &mut Window, &mut App)>;

/// The question itself. `answer` is given whether "Don't ask again" was ticked, and only
/// when the red button is pressed, once the question has closed; Cancel and Escape drop it.
fn ask(
    ending: Ending,
    agents: Vec<SharedString>,
    window: &mut Window,
    cx: &mut App,
    answer: impl FnOnce(bool, &mut Window, &mut App) + 'static,
) {
    let (title, body) = prompt_text(agents.len(), ending);
    let dont_ask = Rc::new(Cell::new(false));
    let answer: Rc<Cell<Option<Answer>>> = Rc::new(Cell::new(Some(Box::new(answer))));
    window.open_alert_dialog(cx, move |alert, _, _| {
        let checkbox_state = dont_ask.clone();
        let (ok_state, answer) = (dont_ask.clone(), answer.clone());
        crate::ui::destructive::confirmation(
            alert.confirm().title(title.clone()).description(
                v_flex()
                    .gap_3()
                    .child(body.clone())
                    .children(agents.iter().cloned())
                    .child(
                        Checkbox::new("dont-ask-close-running-agent")
                            .label("Don't ask again for running agents")
                            .checked(dont_ask.get())
                            .on_click(move |checked, window, _| {
                                checkbox_state.set(*checked);
                                window.refresh();
                            }),
                    ),
            ),
            if agents.len() == 1 {
                "Stop Agent"
            } else {
                "Stop Agents"
            },
            move |window, cx| {
                if let Some(answer) = answer.take() {
                    let dont_ask_again = ok_state.get();
                    // After the question has closed: it then gives the keyboard back to the
                    // terminal it was asked from, and a question that follows opens over
                    // nothing. A window closed meanwhile drops the answer.
                    window.defer(cx, move |window, cx| answer(dont_ask_again, window, cx));
                }
                true
            },
        )
    });
}

/// The agents running in `sessions`, ordered like `sessions`.
fn agents_in(sessions: &[EntityId], agents: &[&AgentSession]) -> Vec<AgentSession> {
    sessions
        .iter()
        .filter_map(|session| agents.iter().find(|agent| agent.session == *session))
        .map(|agent| (*agent).clone())
        .collect()
}

/// How the question lists one agent. A remote agent's status is not tracked.
fn agent_line(agent: &AgentSession, local: bool) -> SharedString {
    let status = if local { agent.status.label() } else { "SSH" };
    format!("{} · {status}", agent.kind.name()).into()
}

/// Title and explanation of the confirmation for `count` agents that `ending` would stop.
fn prompt_text(count: usize, ending: Ending) -> (SharedString, SharedString) {
    let action = match ending {
        Ending::Pane => "Closing this terminal pane",
        Ending::Tab => "Closing this terminal tab",
        Ending::Window => "Closing this window",
        Ending::App => "Quitting Hive",
    };
    if count == 1 {
        return (
            "Stop this agent?".into(),
            format!("{action} stops the agent's current work.").into(),
        );
    }
    let scope = match ending {
        Ending::Pane | Ending::Tab | Ending::Window => "every agent in it",
        Ending::App => "every agent",
    };
    (
        format!("Stop {count} agents?").into(),
        format!("{action} stops the current work of {scope}.").into(),
    )
}

#[cfg(test)]
mod tests;
