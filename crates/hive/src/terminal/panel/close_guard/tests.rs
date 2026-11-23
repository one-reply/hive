// Not a glob import: it would bring in GPUI's `test` attribute in place of std's.
use super::{AgentSession, Ending, agent_line, agents_in, ask, confirm_stopping, prompt_text};
use crate::ui::destructive::testing::{is_open, press, settle, window};
use gpui_kit as gpui;
use gpui_kit::{AppContext as _, EntityId, SharedString, TestAppContext, VisualTestContext};
use hive_core::agent::{AgentKind, AgentStatus};
use std::cell::Cell;
use std::rc::Rc;

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
    let (title, body) = prompt_text(1, Ending::Pane);
    assert_eq!(title.as_ref(), "Stop this agent?");
    assert!(body.contains("terminal pane"));
    let (title, body) = prompt_text(3, Ending::Tab);
    assert_eq!(title.as_ref(), "Stop 3 agents?");
    assert!(body.contains("terminal tab"));
}

#[test]
fn quitting_and_closing_the_window_ask_the_same_question_as_closing_a_tab() {
    let (title, body) = prompt_text(1, Ending::App);
    assert_eq!(title.as_ref(), "Stop this agent?");
    assert_eq!(
        body.as_ref(),
        "Quitting Hive stops the agent's current work."
    );
    let (title, body) = prompt_text(4, Ending::App);
    assert_eq!(title.as_ref(), "Stop 4 agents?");
    assert_eq!(
        body.as_ref(),
        "Quitting Hive stops the current work of every agent."
    );
    let (title, body) = prompt_text(2, Ending::Window);
    assert_eq!(title.as_ref(), "Stop 2 agents?");
    assert_eq!(
        body.as_ref(),
        "Closing this window stops the current work of every agent in it."
    );
}

#[test]
fn an_agent_is_listed_by_its_cli_and_status_and_a_remote_one_as_ssh() {
    let agent = agent(1, AgentKind::ClaudeCode);
    assert_eq!(agent_line(&agent, true).as_ref(), "Claude Code · Working");
    assert_eq!(agent_line(&agent, false).as_ref(), "Claude Code · SSH");
}

/// What became of the action waiting for the answer: how often it ran, with which
/// "Don't ask again", and whether it was dropped.
#[derive(Default)]
struct Outcome {
    ran: Cell<u32>,
    dont_ask_again: Cell<bool>,
    dropped: Cell<bool>,
}

/// Stands for a pending quit: dropped unanswered, it cancels.
struct Pending(Rc<Outcome>);

impl Drop for Pending {
    fn drop(&mut self) {
        self.0.dropped.set(true);
    }
}

fn ask_about(agents: &[&str], cx: &mut VisualTestContext) -> Rc<Outcome> {
    let outcome = Rc::new(Outcome::default());
    let pending = Pending(outcome.clone());
    let agents: Vec<SharedString> = agents.iter().map(|line| (*line).into()).collect();
    cx.update(|window, cx| {
        ask(
            Ending::App,
            agents,
            window,
            cx,
            move |dont_ask_again, _, _| {
                pending.0.ran.set(pending.0.ran.get() + 1);
                pending.0.dont_ask_again.set(dont_ask_again);
            },
        )
    });
    settle(cx);
    outcome
}

#[gpui::test]
fn quitting_waits_for_the_answer_and_enter_is_not_one(cx: &mut TestAppContext) {
    let cx = window(cx);
    let outcome = ask_about(&["Claude Code · Working", "Codex · Idle"], cx);
    assert!(is_open(cx));
    assert_eq!(outcome.ran.get(), 0, "nothing stops before the answer");

    // ⌘Q is followed by Enter out of habit; it must not confirm.
    press("enter", cx);
    assert_eq!(outcome.ran.get(), 0);
    assert!(is_open(cx));
    assert!(!outcome.dropped.get(), "the quit is still waiting");
}

#[gpui::test]
fn the_red_button_lets_the_quit_go_on_once(cx: &mut TestAppContext) {
    let cx = window(cx);
    let outcome = ask_about(&["Claude Code · Working"], cx);

    // Tab reaches the checkbox, Cancel, then the red button; Space presses it.
    cx.simulate_keystrokes("tab tab tab");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    press("space", cx);
    settle(cx);

    assert_eq!(outcome.ran.get(), 1);
    assert!(!outcome.dont_ask_again.get());
    assert!(!is_open(cx));
}

#[gpui::test]
fn dismissing_the_question_cancels_what_waited_for_it(cx: &mut TestAppContext) {
    let cx = window(cx);
    let outcome = ask_about(&["Claude Code · Working"], cx);

    cx.simulate_keystrokes("escape");
    settle(cx);

    assert!(!is_open(cx));
    assert_eq!(outcome.ran.get(), 0, "the agents keep running");
    assert!(
        outcome.dropped.get(),
        "a quit request dropped unanswered is a cancelled quit"
    );
}

/// Stands in for the terminal panel: whatever asks is inside its own update.
struct Panel {
    closed: bool,
}

/// A tab's close button asks from inside the panel's update, and what it hands over closes
/// the panel, which updates it again: with nothing to ask, that may run only once the update
/// that asked is over.
#[gpui::test]
fn with_nothing_to_ask_the_close_runs_after_the_update_that_asked(cx: &mut TestAppContext) {
    let cx = window(cx);
    let panel = cx.update(|_, cx| cx.new(|_| Panel { closed: false }));
    cx.update(|window, cx| {
        panel.update(cx, |_, cx| {
            let asked_from = cx.entity();
            confirm_stopping(Ending::Tab, Vec::new(), window, cx, move |_, cx| {
                asked_from.update(cx, |panel, _| panel.closed = true);
            });
        })
    });
    cx.run_until_parked();
    assert!(cx.read(|cx| panel.read(cx).closed));
}
