//! Template cards change places by dragging one onto another.
use super::*;

/// Width of the card that follows the pointer: the narrowest a card in the library gets.
const PREVIEW_WIDTH: Rems = rems(11.);
const PREVIEW_OPACITY: f32 = 0.9;

/// The card being dragged, drawn under the pointer.
#[derive(Clone)]
struct DraggedTemplate(Template);

impl Render for DraggedTemplate {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w(PREVIEW_WIDTH)
            .border_1()
            .border_color(cx.theme().drag_border)
            .rounded_lg()
            .overflow_hidden()
            .shadow_md()
            .opacity(PREVIEW_OPACITY)
            .bg(cx.theme().sidebar)
            .child(map::miniature(&self.0, cx))
            .child(
                div()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .truncate()
                    .child(self.0.name.clone()),
            )
    }
}

/// Lets `card` of `template` be dragged, and takes another card dropped on it: `on_move`
/// gets the name of the dropped template and of this one, whose place it is to take. A card
/// dropped on itself moves nothing.
pub(super) fn reorderable(
    card: Stateful<Div>,
    template: &Template,
    on_move: impl Fn(&str, &str, &mut App) + 'static,
) -> Stateful<Div> {
    let (hovered, target) = (template.name.clone(), template.name.clone());
    card.on_drag(DraggedTemplate(template.clone()), |drag, _, _, cx| {
        cx.stop_propagation();
        cx.new(|_| drag.clone())
    })
    .drag_over::<DraggedTemplate>(move |style, drag, _, cx| {
        // A card over its own place has nowhere to go.
        if drag.0.name == hovered {
            return style;
        }
        style.border_color(cx.theme().drag_border)
    })
    .on_drop(move |drag: &DraggedTemplate, _, cx| {
        if drag.0.name != target {
            on_move(&drag.0.name, &target, cx);
        }
    })
}

#[cfg(test)]
mod tests {
    // Not a glob import: the parent's `gpui_kit::*` brings in GPUI's `test` attribute.
    use super::reorderable;
    use gpui_kit as gpui;
    use gpui_kit::prelude::*;
    use gpui_kit::{
        Context, MouseButton, Pixels, Point, TestAppContext, VisualTestContext, Window, div, point,
        px,
    };
    use hive_core::pipeline::Template;
    use std::cell::RefCell;
    use std::rc::Rc;

    const CARD: Pixels = px(100.);

    type Moves = Rc<RefCell<Vec<(String, String)>>>;

    /// Cards "A", "B" and "C" in a row, each `CARD` wide, recording what a drop asks for.
    struct Row(Moves);
    impl Render for Row {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().flex().children(["A", "B", "C"].map(|name| {
                let template = Template {
                    name: name.into(),
                    ..Template::default()
                };
                let moves = self.0.clone();
                reorderable(
                    div().id(name).size(CARD),
                    &template,
                    move |name, onto, _| {
                        moves.borrow_mut().push((name.into(), onto.into()));
                    },
                )
            }))
        }
    }

    fn row(cx: &mut TestAppContext) -> (&mut VisualTestContext, Moves) {
        cx.update(gpui_kit::init);
        let moves = Moves::default();
        let recorded = moves.clone();
        let (_, cx) = cx.add_window_view(|_, _| Row(recorded));
        (cx, moves)
    }

    /// The middle of the card at `index`.
    fn card(index: usize) -> Point<Pixels> {
        point(CARD * (index as f32 + 0.5), CARD / 2.)
    }

    fn drag(from: Point<Pixels>, to: Point<Pixels>, cx: &mut VisualTestContext) {
        cx.simulate_mouse_down(from, MouseButton::Left, Default::default());
        cx.simulate_mouse_move(to, MouseButton::Left, Default::default());
        cx.simulate_mouse_up(to, MouseButton::Left, Default::default());
        cx.run_until_parked();
    }

    #[gpui::test]
    fn a_card_dropped_on_another_asks_for_its_place(cx: &mut TestAppContext) {
        let (cx, moves) = row(cx);
        drag(card(0), card(2), cx);
        drag(card(2), card(1), cx);
        assert_eq!(
            *moves.borrow(),
            [("A".into(), "C".into()), ("C".into(), "B".into())]
        );
    }

    #[gpui::test]
    fn a_card_dropped_on_itself_or_outside_the_cards_moves_nothing(cx: &mut TestAppContext) {
        let (cx, moves) = row(cx);
        drag(card(1), card(1) + point(px(20.), px(0.)), cx);
        drag(card(1), card(1) + point(px(0.), CARD * 3.), cx);
        assert!(moves.borrow().is_empty());
    }
}
