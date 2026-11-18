//! Accessible disclosure and file rows share keyboard and pointer activation.
use super::*;
// Mouse, keyboard, and assistive technologies activate the same state transition.
pub(super) fn actionable<V: 'static>(
    row: Stateful<Div>,
    label: impl Into<SharedString>,
    expanded: Option<bool>,
    cx: &Context<V>,
    activate: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> Stateful<Div> {
    let focus = cx.theme().ring;
    let row = row
        .role(Role::Button)
        .aria_label(label)
        .when_some(expanded, |row, open| row.aria_expanded(open))
        .tab_index(0)
        .border_1()
        .border_color(transparent_black())
        .focus_visible(move |s| s.border_color(focus));
    on_activate(row, cx, move |v, _, cx| activate(v, cx))
}

/// Flips a disclosure row between expanded and collapsed.
pub(super) fn toggle<T: std::hash::Hash + Eq>(expanded: &mut HashSet<T>, key: T) {
    if !expanded.remove(&key) {
        expanded.insert(key);
    }
}

#[cfg(test)]
mod tests {
    use super::actionable;
    use gpui_kit as gpui;
    use gpui_kit::prelude::*;
    use gpui_kit::{Context, FocusHandle, TestAppContext, Window, div};

    struct Harness {
        open: bool,
        focus: FocusHandle,
    }
    impl Render for Harness {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            actionable(
                div().id("review").track_focus(&self.focus),
                "QA, in progress",
                Some(self.open),
                cx,
                |v, cx| {
                    v.open = !v.open;
                    cx.notify();
                },
            )
            .child("QA")
        }
    }
    #[gpui::test]
    fn focused_stage_opens_with_enter_and_collapses_with_space(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(|w, cx| {
            let focus = cx.focus_handle();
            focus.focus(w, cx);
            Harness { open: false, focus }
        });
        cx.update(|w, cx| w.draw(cx).clear(cx));
        cx.simulate_keystrokes("enter");
        cx.update(|_, cx| assert!(view.read(cx).open));
        cx.simulate_keystrokes("space");
        cx.update(|_, cx| assert!(!view.read(cx).open));
    }
}
