// Not a glob import: the parent's `gpui_kit::*` brings in GPUI's `test` attribute.
use super::{
    BoxAt, Dragged, Dragging, Ghost, Item, Layout, Load, Reaction, StageChoice, Target, follow,
    land, pointer_at, reaction, target, units,
};
use gpui_kit as gpui;
use gpui_kit::prelude::*;
use gpui_kit::{
    Bounds, Context, DragMoveEvent, MouseButton, Pixels, Point, TestAppContext, VisualTestContext,
    Window, div, point, px, size,
};
use hive_core::pipeline::ReviewKind;

/// The graph of a recipe with three lanes and three reviews, following a drag the way
/// the editor does and recording what each drop asks for. The third review can be moved;
/// the first one's whole card stands in for its findings handle.
struct Graph {
    layout: Layout,
    dragging: Option<Dragging>,
    landed: Vec<(Load, Target)>,
}

impl Render for Graph {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let editor = cx.entity_id();
        let handle = |id: &'static str, at: BoxAt, load| {
            let dragged = Dragged {
                editor,
                load,
                ghost: Ghost::Findings("QA".into()),
            };
            div()
                .id(id)
                .absolute()
                .left(units(at.x))
                .top(units(at.y))
                .w(units(at.w))
                .h(units(at.h))
                .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
        };
        div()
            .relative()
            .w(units(self.layout.width))
            .h(units(self.layout.height))
            .on_drag_move(cx.listener(|graph, event: &DragMoveEvent<Dragged>, _, cx| {
                let editor = cx.entity_id();
                follow(&mut graph.dragging, event, &graph.layout, editor, cx);
            }))
            .on_drop(cx.listener(|graph, dragged: &Dragged, _, cx| {
                let landed = land(&mut graph.dragging, dragged, cx.entity_id());
                graph.landed.extend(landed);
            }))
            .child(handle(
                "review",
                self.layout.reviews[2],
                Load::Move(Item::Review(2)),
            ))
            .child(handle("findings", self.layout.reviews[0], Load::Route(0)))
    }
}

fn graph(cx: &mut TestAppContext) -> (gpui::Entity<Graph>, &mut VisualTestContext) {
    cx.update(gpui_kit::init);
    cx.add_window_view(|_, _| Graph {
        layout: Layout::new(3, &[1, 1, 1]),
        dragging: None,
        landed: Vec::new(),
    })
}

/// A point of the graph, given in layout units, as the window sees it.
fn on_screen((x, y): (f32, f32), cx: &mut VisualTestContext) -> Point<Pixels> {
    let rem = cx.update(|window, _| window.rem_size());
    point(units(x).to_pixels(rem), units(y).to_pixels(rem))
}

fn middle(at: BoxAt) -> (f32, f32) {
    (at.middle(), at.y + at.h / 2.)
}

/// Presses at `from`, drags by way of a point beside it and releases at `to`.
fn drag(from: (f32, f32), to: (f32, f32), cx: &mut VisualTestContext) {
    let (start, beside) = (on_screen(from, cx), on_screen((from.0 + 12., from.1), cx));
    let end = on_screen(to, cx);
    cx.simulate_mouse_down(start, MouseButton::Left, Default::default());
    for at in [beside, end] {
        cx.simulate_mouse_move(at, MouseButton::Left, Default::default());
    }
    cx.simulate_mouse_up(end, MouseButton::Left, Default::default());
    cx.run_until_parked();
}

#[gpui::test]
fn a_stage_dropped_over_a_gap_lands_there_and_one_dropped_elsewhere_stays(cx: &mut TestAppContext) {
    let (view, cx) = graph(cx);
    let layout = Layout::new(3, &[1, 1, 1]);
    let third = middle(layout.reviews[2]);
    // Over the upper half of the first review: the gap above it.
    drag(third, (third.0, layout.reviews[0].y + 5.), cx);
    // Over the planner, and back over its own lower half: nowhere to go.
    drag(third, middle(layout.planner), cx);
    drag(third, (third.0, layout.reviews[2].floor() - 5.), cx);
    // Released past the bottom of the graph, then dragged again.
    drag(third, (third.0, layout.height + 80.), cx);
    drag(third, review_gap(&layout, 1), cx);
    view.read_with(cx, |graph, _| {
        assert_eq!(
            graph.landed,
            [
                (Load::Move(Item::Review(2)), Target::Gap(0)),
                (Load::Move(Item::Review(2)), Target::Gap(1)),
            ]
        );
    });
}

#[gpui::test]
fn findings_dropped_on_a_lane_or_on_development_ask_for_that_route(cx: &mut TestAppContext) {
    let (view, cx) = graph(cx);
    let layout = Layout::new(3, &[1, 1, 1]);
    let findings = middle(layout.reviews[0]);
    drag(findings, middle(layout.lanes[2]), cx);
    drag(findings, middle(layout.build), cx);
    drag(findings, review_gap(&layout, 3), cx);
    view.read_with(cx, |graph, _| {
        assert_eq!(
            graph.landed,
            [
                (Load::Route(0), Target::Lane(Some(2))),
                (Load::Route(0), Target::Lane(None)),
            ]
        );
    });
}

#[test]
fn the_pointer_is_measured_in_layout_units_whatever_the_canvas_is_scaled_to() {
    let bounds = Bounds::new(point(px(100.), px(40.)), size(px(600.), px(900.)));
    let at = pointer_at(bounds, point(px(250.), px(340.)), 300.);
    assert_eq!(at, Some((75., 150.)));
    let collapsed = Bounds::new(point(px(0.), px(0.)), size(px(0.), px(0.)));
    assert_eq!(pointer_at(collapsed, point(px(5.), px(5.)), 300.), None);
}

/// The middle of the gap before position `gap` among the reviews.
fn review_gap(layout: &Layout, gap: usize) -> (f32, f32) {
    let line = layout.review_gap_line(gap).unwrap();
    (line.middle(), line.y)
}

fn lane_gap(layout: &Layout, gap: usize) -> (f32, f32) {
    let line = layout.lane_gap_line(gap).unwrap();
    (line.x, line.y + line.h / 2.)
}

#[test]
fn a_stage_lands_in_any_gap_of_its_own_row_but_the_two_beside_it() {
    let layout = Layout::new(3, &[1, 1, 1]);
    let landed = |load, at| target(&layout, load, at);
    let review = Load::Move(Item::Review(1));
    assert_eq!(landed(review, review_gap(&layout, 0)), Some(Target::Gap(0)));
    assert_eq!(landed(review, review_gap(&layout, 1)), None);
    assert_eq!(landed(review, review_gap(&layout, 2)), None);
    assert_eq!(landed(review, review_gap(&layout, 3)), Some(Target::Gap(3)));
    assert_eq!(landed(review, lane_gap(&layout, 0)), None);

    let lane = Load::Move(Item::Lane(0));
    assert_eq!(landed(lane, lane_gap(&layout, 0)), None);
    assert_eq!(landed(lane, lane_gap(&layout, 1)), None);
    assert_eq!(landed(lane, lane_gap(&layout, 2)), Some(Target::Gap(2)));
    assert_eq!(landed(lane, review_gap(&layout, 0)), None);
    assert_eq!(
        landed(Load::Move(Item::Planner), review_gap(&layout, 0)),
        None
    );
}

#[test]
fn a_review_lands_beside_another_over_its_side_and_leaves_its_group_into_a_gap() {
    // Review 0 alone, reviews 1 and 2 together, review 3 alone.
    let layout = Layout::new(1, &[1, 2, 1]);
    let side = |review: usize, after: bool| {
        let card = layout.reviews[review];
        let x = if after {
            card.x + card.w - 5.
        } else {
            card.x + 5.
        };
        (x, card.y + card.h / 2.)
    };
    let beside = |review, after| Some(Target::Beside { review, after });
    let alone = Load::Move(Item::Review(3));
    assert_eq!(target(&layout, alone, side(0, true)), beside(0, true));
    assert_eq!(target(&layout, alone, side(1, false)), beside(1, false));
    // Over the middle of a card the nearest gap between rows counts, as before.
    let card = layout.reviews[0];
    let middle = (card.middle(), card.y + 5.);
    assert_eq!(target(&layout, alone, middle), Some(Target::Gap(0)));

    // A member of the group already stands after its neighbour; before it is a move.
    let member = Load::Move(Item::Review(2));
    assert_eq!(target(&layout, member, side(1, true)), None);
    assert_eq!(target(&layout, member, side(1, false)), beside(1, false));
    // It leaves the group into the gap above or below its own row.
    assert_eq!(
        target(&layout, member, review_gap(&layout, 1)),
        Some(Target::Gap(1))
    );
    assert_eq!(
        target(&layout, member, review_gap(&layout, 2)),
        Some(Target::Gap(2))
    );
    // A review alone on its row stays in the gaps around that row.
    assert_eq!(target(&layout, alone, review_gap(&layout, 2)), None);
    assert_eq!(target(&layout, alone, review_gap(&layout, 3)), None);

    let new = Load::Add(StageChoice::Review(ReviewKind::Security));
    assert_eq!(target(&layout, new, side(3, true)), beside(3, true));
    // A lane has no place among the reviews, beside one or between them.
    assert_eq!(
        target(&layout, Load::Move(Item::Lane(0)), side(0, true)),
        None
    );
}

#[test]
fn a_new_stage_lands_in_every_gap_of_its_row() {
    let layout = Layout::new(1, &[1, 1]);
    let review = Load::Add(StageChoice::Review(ReviewKind::Security));
    for gap in 0..=2 {
        assert_eq!(
            target(&layout, review, review_gap(&layout, gap)),
            Some(Target::Gap(gap))
        );
    }
    let lane = Load::Add(StageChoice::Lane);
    for gap in 0..=1 {
        assert_eq!(
            target(&layout, lane, lane_gap(&layout, gap)),
            Some(Target::Gap(gap))
        );
    }
    assert_eq!(target(&layout, lane, review_gap(&layout, 1)), None);
}

#[test]
fn findings_land_on_a_lane_or_on_development_for_every_affected_lane() {
    let layout = Layout::new(2, &[1]);
    let middle = |at: super::BoxAt| (at.middle(), at.y + at.h / 2.);
    let findings = Load::Route(0);
    assert_eq!(
        target(&layout, findings, middle(layout.lanes[1])),
        Some(Target::Lane(Some(1)))
    );
    assert_eq!(
        target(&layout, findings, middle(layout.build)),
        Some(Target::Lane(None))
    );
    assert_eq!(target(&layout, findings, middle(layout.planner)), None);
    assert_eq!(target(&layout, findings, review_gap(&layout, 0)), None);
}

#[test]
fn cards_answer_the_drag_they_take_part_in() {
    let moving = Some(Dragging {
        load: Load::Move(Item::Review(1)),
        target: Some(Target::Gap(0)),
    });
    assert_eq!(reaction(moving, Item::Review(1)), Reaction::Lifted);
    assert_eq!(reaction(moving, Item::Review(0)), Reaction::None);
    assert_eq!(reaction(moving, Item::Lane(0)), Reaction::None);

    let routing = Some(Dragging {
        load: Load::Route(0),
        target: Some(Target::Lane(Some(1))),
    });
    assert_eq!(reaction(routing, Item::Lane(1)), Reaction::Aimed);
    assert_eq!(reaction(routing, Item::Lane(0)), Reaction::Candidate);
    assert_eq!(reaction(routing, Item::Development), Reaction::Candidate);
    assert_eq!(reaction(routing, Item::Review(0)), Reaction::None);
    assert_eq!(reaction(None, Item::Lane(1)), Reaction::None);
}
