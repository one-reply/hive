use super::ROW_OVERDRAW;
use crate::ui::list_scroll::replace_rows;
use gpui_kit as gpui;
use gpui_kit::{
    Context, IntoElement, ListAlignment, ListState, Render, ScrollDelta, ScrollEvent, Styled as _,
    TestAppContext, Window, div, list, point, px,
};

struct ResultList(ListState);

impl Render for ResultList {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // The first wrapped line fills the viewport. Accepting both appends two more.
        list(self.0.clone(), |ix, _, _| {
            div()
                .w_full()
                .h(px(if ix == 0 { 160. } else { 80. }))
                .into_any_element()
        })
        .w(px(320.))
        .h(px(160.))
    }
}

#[gpui::test]
fn scroll_reaches_the_appended_result_rows(cx: &mut TestAppContext) {
    let state = ListState::new(1, ListAlignment::Top, ROW_OVERDRAW);
    let (_, cx) = cx.add_window_view(|_, _| ResultList(state.clone()));
    cx.update(|window, cx| window.draw(cx).clear(cx));

    replace_rows(&state, &[0], &[0, 1, 2]);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    for _ in 0..2 {
        cx.simulate_event(ScrollEvent {
            position: point(px(40.), px(40.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-80.))),
            ..Default::default()
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
    let last = state
        .bounds_for_item(2)
        .expect("last result row is measured");
    assert_eq!(last.bottom(), state.viewport_bounds().bottom());
}
