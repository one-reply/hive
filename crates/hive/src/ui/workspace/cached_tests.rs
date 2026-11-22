//! What caching a panel relies on in the pinned GPUI: a cached view is reused while only its
//! neighbours ask for frames, and is rebuilt as soon as it, or a view drawn inside it, does.
use super::cached;
use gpui_kit as gpui;
use gpui_kit::prelude::*;
use gpui_kit::{Context, Entity, TestAppContext, VisualTestContext, Window, div, px};
use std::{cell::Cell, rc::Rc};

type Builds = Rc<Cell<u32>>;

/// A view that counts how often it is built.
struct Leaf(Builds);

impl Render for Leaf {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.0.set(self.0.get() + 1);
        div().size_full()
    }
}

/// The cached panel, with a screen of its own inside it.
struct Panel {
    builds: Builds,
    screen: Entity<Leaf>,
}

impl Render for Panel {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.builds.set(self.builds.get() + 1);
        div().size_full().child(self.screen.clone())
    }
}

struct Root {
    panel: Entity<Panel>,
    neighbour: Entity<Leaf>,
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let slot = || div().w(px(200.)).h_full();
        div()
            .flex()
            .size_full()
            .child(slot().child(cached(&self.panel)))
            .child(slot().child(self.neighbour.clone()))
    }
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

#[gpui::test]
fn a_cached_panel_is_reused_for_a_neighbours_frame_and_rebuilt_for_its_own_screen(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_kit::init);
    let builds: [Builds; 3] = Default::default();
    let [panel_builds, screen_builds, neighbour_builds] = builds.clone();
    let (root, cx) = cx.add_window_view(|_, cx| {
        let screen = cx.new(|_| Leaf(screen_builds));
        Root {
            panel: cx.new(|_| Panel {
                builds: panel_builds,
                screen,
            }),
            neighbour: cx.new(|_| Leaf(neighbour_builds)),
        }
    });
    let (panel, neighbour) = root.read_with(cx, |root, _| (root.panel.clone(), root.neighbour.clone()));
    let screen = panel.read_with(cx, |panel, _| panel.screen.clone());
    let built = || builds.clone().map(|count| count.get());
    draw(cx);
    let [panel_first, screen_first, neighbour_first] = built();
    assert!(panel_first >= 1 && screen_first >= 1);

    // A frame the neighbour asks for: a terminal printing, the spinner of a working agent.
    neighbour.update(cx, |_, cx| cx.notify());
    draw(cx);
    assert_eq!(built(), [panel_first, screen_first, neighbour_first + 1]);

    // A frame a screen inside the panel asks for: its clock, its spinner, its progress.
    screen.update(cx, |_, cx| cx.notify());
    draw(cx);
    assert_eq!(built(), [panel_first + 1, screen_first + 1, neighbour_first + 2]);

    panel.update(cx, |_, cx| cx.notify());
    draw(cx);
    assert_eq!(built()[0], panel_first + 2);
}
