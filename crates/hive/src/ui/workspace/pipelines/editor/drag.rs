//! Dragging on the recipe canvas: a stage into a gap between its neighbours, a new stage out
//! of the palette, and a review's findings onto the lane that fixes them.
//!
//! The canvas has one drop area, the graph itself. Where a load would land is worked out
//! from the pointer position by [`target`], so the rule is the same for the line the user
//! sees and for the drop.
use super::super::map::{BoxAt, units};
use super::nodes::Face;
use super::*;
use crate::ui::hover::SmoothHoverBg as _;
use gpui_kit::component::tooltip::Tooltip;

/// Side of the round handle a review's findings are dragged from, in layout units.
const PORT: f32 = 10.;
const GHOST_OPACITY: f32 = 0.9;
/// A stage being moved stays in its place, faded, until it is dropped.
const LIFTED_OPACITY: f32 = 0.4;
/// A lane that could take the dragged findings, before the pointer is over it.
const CANDIDATE_ALPHA: f32 = 0.5;

/// What is being dragged.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Load {
    /// A lane or a review changing its place.
    Move(Item),
    /// A new stage from the palette.
    Add(StageChoice),
    /// The findings of this review, on their way to the lane that fixes them.
    Route(usize),
}

/// The stages a dropped stage can stand among.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Row {
    Lanes,
    Reviews,
}

impl Load {
    /// The row a stage is dropped into and, for a stage already on the canvas, its place
    /// there. `None` for a load that is not a stage looking for a place.
    fn row(self) -> Option<(Row, Option<usize>)> {
        match self {
            Self::Move(Item::Lane(i)) => Some((Row::Lanes, Some(i))),
            Self::Move(Item::Review(i)) => Some((Row::Reviews, Some(i))),
            Self::Add(StageChoice::Lane) => Some((Row::Lanes, None)),
            Self::Add(StageChoice::Review(_)) => Some((Row::Reviews, None)),
            Self::Move(Item::Planner | Item::Development) | Self::Route(_) => None,
        }
    }
}

/// Where a load lands if it is dropped now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Target {
    /// The gap before this position of the load's row.
    Gap(usize),
    /// A lane by position, or every lane the findings affect.
    Lane(Option<usize>),
}

/// Where `load` would land if it were dropped at `at`, a point of the graph in layout units.
pub(super) fn target(layout: &Layout, load: Load, at: (f32, f32)) -> Option<Target> {
    if let Load::Route(_) = load {
        return match layout.lanes.iter().position(|lane| lane.contains(at)) {
            Some(lane) => Some(Target::Lane(Some(lane))),
            None => layout.build.contains(at).then_some(Target::Lane(None)),
        };
    }
    let (row, place) = load.row()?;
    let gap = match row {
        Row::Lanes => layout.lane_gap_at(at),
        Row::Reviews => layout.review_gap_at(at),
    }?;
    // The two gaps beside a stage are where it already stands.
    let stays = place.is_some_and(|place| gap == place || gap == place + 1);
    (!stays).then_some(Target::Gap(gap))
}

/// The drag over the canvas as of the last pointer move.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Dragging {
    pub(super) load: Load,
    pub(super) target: Option<Target>,
}

/// The pointer as a point of the graph in layout units. The graph is `width` units wide and
/// drawn across `bounds`; `None` before it has a size.
fn pointer_at(bounds: Bounds<Pixels>, pointer: Point<Pixels>, width: f32) -> Option<(f32, f32)> {
    let scale = f32::from(bounds.size.width) / width;
    let offset = pointer - bounds.origin;
    (scale.is_finite() && scale > 0.)
        .then(|| (f32::from(offset.x) / scale, f32::from(offset.y) / scale))
}

/// Follows a drag started in `editor` as the pointer moves over the graph. Returns whether
/// what the canvas shows changed.
fn follow(
    dragging: &mut Option<Dragging>,
    event: &DragMoveEvent<Dragged>,
    layout: &Layout,
    editor: EntityId,
    cx: &App,
) -> bool {
    let dragged = event.drag(cx);
    if dragged.editor != editor {
        return false;
    }
    let Some(at) = pointer_at(event.bounds, event.event.position, layout.width) else {
        return false;
    };
    let now = Some(Dragging {
        load: dragged.load,
        target: target(layout, dragged.load, at),
    });
    let changed = *dragging != now;
    *dragging = now;
    changed
}

/// Ends the drag on the graph: what was dropped and where, when it has somewhere to land.
fn land(
    dragging: &mut Option<Dragging>,
    dragged: &Dragged,
    editor: EntityId,
) -> Option<(Load, Target)> {
    let Dragging { load, target } = dragging.take()?;
    (dragged.editor == editor && dragged.load == load).then_some((load, target?))
}

/// How a stage card answers the drag in progress.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Reaction {
    None,
    /// The stage being moved.
    Lifted,
    /// It could take the findings being dragged.
    Candidate,
    /// It takes them if they are dropped now.
    Aimed,
}

pub(super) fn reaction(dragging: Option<Dragging>, item: Item) -> Reaction {
    let Some(Dragging { load, target }) = dragging else {
        return Reaction::None;
    };
    let lane = match item {
        Item::Lane(i) => Some(Some(i)),
        Item::Development => Some(None),
        Item::Planner | Item::Review(_) => None,
    };
    match (load, lane) {
        (Load::Move(moved), _) if moved == item => Reaction::Lifted,
        (Load::Route(_), Some(lane)) if target == Some(Target::Lane(lane)) => Reaction::Aimed,
        (Load::Route(_), Some(_)) => Reaction::Candidate,
        _ => Reaction::None,
    }
}

/// What the card under the pointer shows.
#[derive(Clone)]
pub(super) enum Ghost {
    /// A stage card, in the box it has on the canvas when it is already there.
    Stage(Face, Option<BoxAt>),
    /// The findings of the review with this name.
    Findings(SharedString),
}

/// The payload of a drag and the card GPUI draws under the pointer.
#[derive(Clone)]
pub(super) struct Dragged {
    pub(super) editor: EntityId,
    pub(super) load: Load,
    pub(super) ghost: Ghost,
}

impl Render for Dragged {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ghost = h_flex()
            .px_2()
            .gap_2()
            .border_1()
            .border_color(cx.theme().drag_border)
            .bg(cx.theme().sidebar)
            .shadow_md()
            .opacity(GHOST_OPACITY);
        match &self.ghost {
            Ghost::Stage(face, at) => ghost
                .rounded_lg()
                .map(|ghost| match at {
                    Some(at) => ghost.w(units(at.w)).h(units(at.h)),
                    None => ghost.py_1p5(),
                })
                .children(face.content(cx)),
            Ghost::Findings(review) => ghost
                .py_1()
                .rounded_full()
                .child(
                    Icon::new(Lucide::CornerUpLeft)
                        .size_3()
                        .text_color(cx.theme().warning),
                )
                .child(div().text_xs().child(format!("Findings of {review}"))),
        }
    }
}

impl TemplateEditor {
    /// Forgets a drag that ended anywhere but on the graph. GPUI redraws the window when a
    /// drag ends, so the next render is the place to notice.
    pub(super) fn settle_drag(&mut self, cx: &App) {
        if !cx.has_active_drag() {
            self.dragging = None;
        }
    }

    pub(super) fn drag_moved(&mut self, event: &DragMoveEvent<Dragged>, cx: &mut Context<Self>) {
        let layout = self.layout();
        if follow(&mut self.dragging, event, &layout, cx.entity_id(), cx) {
            cx.notify();
        }
    }

    pub(super) fn dropped(&mut self, dragged: &Dragged, w: &mut Window, cx: &mut Context<Self>) {
        cx.notify();
        let Some((load, target)) = land(&mut self.dragging, dragged, cx.entity_id()) else {
            return;
        };
        match (load, target) {
            (Load::Move(item), Target::Gap(before)) => self.move_to(item, before, w, cx),
            (Load::Add(stage), Target::Gap(before)) => self.add(stage, Some(before), w, cx),
            (Load::Route(review), Target::Lane(None)) => self.route(review, None),
            (Load::Route(review), Target::Lane(Some(lane))) => {
                if let Some(lane) = self.template.lanes.get(lane) {
                    self.route(review, Some(lane.name.clone()));
                }
            }
            (Load::Route(_), Target::Gap(_)) | (Load::Move(_) | Load::Add(_), Target::Lane(_)) => {}
        }
    }

    /// The line across the gap the dragged stage would land in.
    pub(super) fn insertion_line(&self, layout: &Layout, cx: &App) -> Option<Div> {
        let Dragging {
            load,
            target: Some(Target::Gap(gap)),
        } = self.dragging?
        else {
            return None;
        };
        let at = match load.row()?.0 {
            Row::Lanes => layout.lane_gap_line(gap),
            Row::Reviews => layout.review_gap_line(gap),
        }?;
        Some(
            div()
                .absolute()
                .left(units(at.x))
                .top(units(at.y))
                .w(units(at.w))
                .h(units(at.h))
                .rounded_full()
                .bg(cx.theme().drag_border),
        )
    }

    /// The handle on a review's edge, where its return arrow starts: dragging it onto a lane
    /// sends the review's findings there.
    pub(super) fn route_port(
        &self,
        review: usize,
        layout: &Layout,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let at = layout.reviews[review];
        let id = SharedString::from(format!("pipeline-route-port-{review}"));
        let hover = Hover::new(id.clone(), w, cx);
        let dragged = Dragged {
            editor: cx.entity_id(),
            load: Load::Route(review),
            ghost: Ghost::Findings(self.template.reviews[review].name.clone().into()),
        };
        div()
            .id(id)
            .absolute()
            .left(units(at.x + at.w - PORT / 2.))
            .top(units(at.y + at.h / 2. - PORT / 2.))
            .size(units(PORT))
            .rounded_full()
            .border_1()
            .border_color(cx.theme().warning)
            .cursor_grab()
            .tooltip(|window, cx| {
                Tooltip::new("Drag to a lane to send findings there").build(window, cx)
            })
            .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
            .hover_bg(&hover, cx.theme().background, cx.theme().warning)
            .into_any_element()
    }
}

/// The border and fade of a stage card while something is dragged over the canvas.
pub(super) fn react(card: Stateful<Div>, reaction: Reaction, cx: &App) -> Stateful<Div> {
    let warning = cx.theme().warning;
    match reaction {
        Reaction::None => card,
        Reaction::Lifted => card.opacity(LIFTED_OPACITY),
        Reaction::Candidate => card.border_color(warning.alpha(CANDIDATE_ALPHA)),
        Reaction::Aimed => card.border_2().border_color(warning),
    }
}

#[cfg(test)]
mod tests;
