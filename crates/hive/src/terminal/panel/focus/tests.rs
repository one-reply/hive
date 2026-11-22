//! The keyboard after a tab or pane comes to the front, in a real dock: the dock reports a
//! display on the tick after the change, which is where the keyboard used to be taken.
// Not a glob import of the panel: it would bring in GPUI's `test` attribute in place of std's.
use super::super::{PaneTree, TerminalPanel, TerminalView, TerminalViewEvent};
use super::{Reveal, takes_focus};
use crate::state::terminal_session::TerminalSession;
use gpui_kit as gpui;
use gpui_kit::component::dock::{DockArea, DockPlacement, panel_handle};
use gpui_kit::prelude::*;
use gpui_kit::{
    Axis, Context, Entity, FocusHandle, TestAppContext, VisualTestContext, Window, div,
};
use hive_core::host::Host;
use std::collections::HashMap;

#[test]
fn a_tab_the_user_asked_for_takes_the_keyboard_wherever_it_was() {
    assert!(takes_focus(Reveal::Requested, false));
    assert!(takes_focus(Reveal::Requested, true));
}

#[test]
fn a_tab_shown_on_its_own_leaves_the_keyboard_where_the_user_is_typing() {
    // An agent's tab closed after `/exit` while the commit message was being written.
    assert!(!takes_focus(Reveal::Unprompted, false));
}

#[test]
fn a_tab_shown_on_its_own_keeps_the_keyboard_that_was_already_in_its_terminals() {
    // The closed tab or pane had the keyboard: it must not stay in something gone.
    assert!(takes_focus(Reveal::Unprompted, true));
}

/// A window with a center dock and a field outside it, standing in for the commit message.
struct Stage {
    dock: Entity<DockArea>,
    elsewhere: FocusHandle,
}

impl Render for Stage {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // The dock is not drawn: its tab bar reads application state these tests do not set
        // up, and who holds the keyboard does not depend on a frame.
        div().size_full().track_focus(&self.elsewhere)
    }
}

fn stage(cx: &mut TestAppContext) -> (Entity<Stage>, &mut VisualTestContext) {
    cx.update(gpui_kit::init);
    cx.add_window_view(|window, cx| Stage {
        dock: cx.new(|cx| DockArea::new("focus-tests", None, window, cx)),
        elsewhere: cx.focus_handle(),
    })
}

fn pane(cx: &mut VisualTestContext) -> Entity<TerminalView> {
    // Who holds the keyboard does not depend on a shell, so none is started.
    let session = cx.new(|_| TerminalSession::starting_for_test());
    // No worktree: the tab then has no project to select, which needs the application state.
    cx.update(|window, cx| cx.new(|cx| TerminalView::with_session(session, None, window, cx)))
}

/// Opens a tab of `panes` side by side, the first one active, the way `reveal` says.
fn open_tab(
    stage: &Entity<Stage>,
    panes: usize,
    reveal: Reveal,
    cx: &mut VisualTestContext,
) -> (Entity<TerminalPanel>, Vec<Entity<TerminalView>>) {
    let views: Vec<_> = (0..panes).map(|_| pane(cx)).collect();
    let dock = cx.read(|cx| stage.read(cx).dock.clone());
    let panel = cx.update(|window, cx| {
        let panel = cx.new(|cx| {
            let mut tree = PaneTree::Leaf(views[0].clone());
            for pair in views.windows(2) {
                tree.split(&pair[0], pair[1].clone(), Axis::Horizontal);
            }
            let mut panel = TerminalPanel {
                host: Host::Local,
                panes: tree,
                active: views[0].clone(),
                pane_subscriptions: HashMap::new(),
                area: dock.downgrade(),
                group: None,
                alone: false,
                split_sizes: HashMap::new(),
                reveal,
            };
            for view in &views {
                panel.watch_pane(view, window, cx);
            }
            panel
        });
        dock.update(cx, |dock, cx| {
            dock.add_panel_view(
                panel_handle(panel.clone()),
                DockPlacement::Center,
                None,
                window,
                cx,
            )
        });
        panel
    });
    cx.run_until_parked();
    (panel, views)
}

fn focus(handle: &FocusHandle, cx: &mut VisualTestContext) {
    cx.update(|window, cx| handle.focus(window, cx));
}

fn focus_elsewhere(stage: &Entity<Stage>, cx: &mut VisualTestContext) -> FocusHandle {
    let elsewhere = cx.read(|cx| stage.read(cx).elsewhere.clone());
    focus(&elsewhere, cx);
    elsewhere
}

fn is_focused(handle: &FocusHandle, cx: &mut VisualTestContext) -> bool {
    cx.update(|window, _| handle.is_focused(window))
}

fn handle_of(view: &Entity<TerminalView>, cx: &mut VisualTestContext) -> FocusHandle {
    cx.read(|cx| view.read(cx).focus_handle().clone())
}

/// The shell or agent of `view` ended on its own, as after `/exit`.
fn exit(view: &Entity<TerminalView>, cx: &mut VisualTestContext) {
    cx.update(|_, cx| view.update(cx, |_, cx| cx.emit(TerminalViewEvent::Exited)));
    cx.run_until_parked();
}

#[gpui::test]
fn a_tab_opened_for_the_user_takes_the_keyboard_from_wherever_it_is(cx: &mut TestAppContext) {
    let (stage, cx) = stage(cx);
    let elsewhere = focus_elsewhere(&stage, cx);

    let (_, panes) = open_tab(&stage, 1, Reveal::Requested, cx);

    assert!(is_focused(&handle_of(&panes[0], cx), cx));
    assert!(!is_focused(&elsewhere, cx));
}

#[gpui::test]
fn an_agent_tab_that_closes_on_its_own_leaves_the_keyboard_in_the_field_being_typed_in(
    cx: &mut TestAppContext,
) {
    let (stage, cx) = stage(cx);
    let (_, shell) = open_tab(&stage, 1, Reveal::Requested, cx);
    let (_, agent) = open_tab(&stage, 1, Reveal::Requested, cx);
    // The agent's tab is displayed; the user writes a commit message.
    let elsewhere = focus_elsewhere(&stage, cx);

    exit(&agent[0], cx);

    assert!(
        is_focused(&elsewhere, cx),
        "the shell's tab is displayed now, and must not take what is being typed"
    );
    assert!(!is_focused(&handle_of(&shell[0], cx), cx));
}

#[gpui::test]
fn a_tab_that_closes_with_the_keyboard_in_it_hands_the_keyboard_to_the_tab_shown_next(
    cx: &mut TestAppContext,
) {
    let (stage, cx) = stage(cx);
    let (_, shell) = open_tab(&stage, 1, Reveal::Requested, cx);
    let (_, agent) = open_tab(&stage, 1, Reveal::Requested, cx);
    assert!(is_focused(&handle_of(&agent[0], cx), cx));

    exit(&agent[0], cx);

    assert!(is_focused(&handle_of(&shell[0], cx), cx));
}

#[gpui::test]
fn a_tab_the_user_closes_from_outside_it_gives_the_keyboard_to_the_tab_shown_next(
    cx: &mut TestAppContext,
) {
    let (stage, cx) = stage(cx);
    let (_, shell) = open_tab(&stage, 1, Reveal::Requested, cx);
    let (agent_tab, _) = open_tab(&stage, 1, Reveal::Requested, cx);
    // A click on the tab's close button takes the keyboard out of the tab first.
    focus_elsewhere(&stage, cx);

    cx.update(|window, cx| {
        agent_tab.update(cx, |tab, cx| tab.close(Reveal::Requested, window, cx))
    });
    cx.run_until_parked();

    assert!(is_focused(&handle_of(&shell[0], cx), cx));
}

#[gpui::test]
fn a_tab_an_agent_opens_takes_the_keyboard_only_from_the_tab_it_covers(cx: &mut TestAppContext) {
    let (stage, cx) = stage(cx);
    let (_, shell) = open_tab(&stage, 1, Reveal::Requested, cx);
    assert!(is_focused(&handle_of(&shell[0], cx), cx));

    // The user was in the tab the helper's tab now covers.
    let (_, helper) = open_tab(&stage, 1, Reveal::Unprompted, cx);
    assert!(is_focused(&handle_of(&helper[0], cx), cx));

    // The user is typing outside the terminals.
    let elsewhere = focus_elsewhere(&stage, cx);
    let (_, second) = open_tab(&stage, 1, Reveal::Unprompted, cx);
    assert!(is_focused(&elsewhere, cx));
    assert!(!is_focused(&handle_of(&second[0], cx), cx));
}

#[gpui::test]
fn a_request_for_the_keyboard_is_spent_on_the_first_display(cx: &mut TestAppContext) {
    let (stage, cx) = stage(cx);
    let (first, panes) = open_tab(&stage, 1, Reveal::Requested, cx);
    let (second, _) = open_tab(&stage, 1, Reveal::Requested, cx);
    let elsewhere = focus_elsewhere(&stage, cx);

    // The second tab goes away without the user: the first is displayed again.
    cx.update(|window, cx| second.update(cx, |tab, cx| tab.close(Reveal::Unprompted, window, cx)));
    cx.run_until_parked();

    assert_eq!(cx.read(|cx| first.read(cx).reveal), Reveal::Unprompted);
    assert!(is_focused(&elsewhere, cx));
    assert!(!is_focused(&handle_of(&panes[0], cx), cx));
}

#[gpui::test]
fn a_pane_that_exits_on_its_own_leaves_the_keyboard_in_the_field_being_typed_in(
    cx: &mut TestAppContext,
) {
    let (stage, cx) = stage(cx);
    let (tab, panes) = open_tab(&stage, 2, Reveal::Requested, cx);
    // The agent's pane was the last one used in this tab; the user moved on.
    let elsewhere = focus_elsewhere(&stage, cx);

    exit(&panes[0], cx);

    assert!(cx.read(|cx| tab.read(cx).active == panes[1]));
    assert!(
        is_focused(&elsewhere, cx),
        "the neighbor becomes the tab's active pane without taking the keyboard"
    );
}

#[gpui::test]
fn a_pane_that_exits_with_the_keyboard_in_it_hands_the_keyboard_to_its_neighbor(
    cx: &mut TestAppContext,
) {
    let (stage, cx) = stage(cx);
    let (_, panes) = open_tab(&stage, 2, Reveal::Requested, cx);
    assert!(is_focused(&handle_of(&panes[0], cx), cx));

    exit(&panes[0], cx);

    assert!(is_focused(&handle_of(&panes[1], cx), cx));
}

#[gpui::test]
fn a_pane_the_user_closes_from_outside_it_gives_the_keyboard_to_its_neighbor(
    cx: &mut TestAppContext,
) {
    let (stage, cx) = stage(cx);
    let (tab, panes) = open_tab(&stage, 2, Reveal::Requested, cx);
    // A click on the pane's close button takes the keyboard out of the pane first.
    focus_elsewhere(&stage, cx);

    cx.update(|window, cx| {
        tab.update(cx, |tab, cx| {
            tab.close_pane(&panes[0], Reveal::Requested, window, cx)
        })
    });

    assert!(is_focused(&handle_of(&panes[1], cx), cx));
}
