use super::super::Sidebar;
use crate::state::{projects::Projects, store::Store};
use crate::ui::hover::Hover;
use gpui_kit as gpui;
use gpui_kit::prelude::*;
use gpui_kit::{
    Context, Entity, IntoElement, MouseButton, Render, TestAppContext, Window, div, point, px,
};
use hive_core::{host::Host, state::StateError};
use std::collections::{HashMap, HashSet};

struct Harness {
    sidebar: Entity<Sidebar>,
    selected: bool,
    selections: usize,
}

impl Render for Harness {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected;
        let strip = self.sidebar.update(cx, |sidebar, cx| {
            let hover = Hover::new("test-strip", window, cx);
            sidebar.render_strip(
                &[],
                &(Host::Local, "/project".into()),
                &hover,
                0.,
                selected,
                cx,
            )
        });
        div()
            .id("card")
            .w(px(300.))
            .h(px(100.))
            .child(strip)
            .on_click(cx.listener(|this, _, _, cx| {
                this.selections += 1;
                this.selected = true;
                cx.notify();
            }))
    }
}

#[gpui::test]
fn inactive_strip_selects_card_then_active_strip_only_toggles_agents(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let store = cx.new(|cx| {
        Store::new(
            Err(StateError::Io {
                operation: "test",
                path: "/unused".into(),
                source: std::io::Error::other("persistence disabled for test"),
            }),
            cx,
        )
    });
    let projects = cx.new(|cx| Projects::new(&store, cx));
    let sidebar = cx.new(|cx| Sidebar {
        projects,
        store: store.clone(),
        collapsed: HashSet::new(),
        toggled: HashMap::new(),
        agents_open: HashSet::new(),
        working: crate::ui::stepped_spinner::SteppedSpinner::new(cx),
        _observers: [
            cx.observe(&store, |_, _, _| {}),
            cx.observe(&store, |_, _, _| {}),
            cx.observe(&store, |_, _, _| {}),
            cx.observe(&store, |_, _, _| {}),
        ],
    });
    let (view, cx) = cx.add_window_view(|_, _| Harness {
        sidebar: sidebar.clone(),
        selected: false,
        selections: 0,
    });
    for expected_open in [false, true, false] {
        cx.update(|window, cx| {
            window.activate_window();
            window.draw(cx).clear(cx);
        });
        let position = point(px(10.), px(10.));
        cx.simulate_mouse_down(position, MouseButton::Left, Default::default());
        cx.simulate_mouse_up(position, MouseButton::Left, Default::default());
        cx.run_until_parked();
        cx.read(|cx| {
            assert_eq!(view.read(cx).selections, 1);
            assert_eq!(
                sidebar
                    .read(cx)
                    .agents_open
                    .contains(&(Host::Local, "/project".into())),
                expected_open
            );
        });
    }
}
